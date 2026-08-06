//! Front-end orchestration at the CLI edge: discover inputs (IO lives here,
//! never in core), inject environment/run-id/World, and run parse → bind →
//! lower for every feature.
//!
//! `--dry-run` is a *validation gate*: every discovered feature is validated
//! regardless of `--tags` — the filter only selects what an execution would
//! run (and what the stats report as selected).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use proef_core::analyze::validate_artifact;
use proef_core::bind;
use proef_core::diag::{Diag, FrontError};
use proef_core::emit::{self, Artifact};
use proef_core::error::CoreError;
use proef_core::feature::{self, FeatureFile};
use proef_core::lower::{self, LowerCtx, LoweredScenario};
use proef_core::pack::{self, PackSet, PackSource};
use proef_core::resolve::ResolveMode;
use proef_core::world::{GlobalStore, World};

use crate::registry;

/// One fully-processed feature.
pub struct LoadedFeature {
    /// The parsed feature (tags, source).
    pub file: FeatureFile,
    /// Processed scenarios, in authored order.
    pub scenarios: Vec<ProcessedScenario>,
}

/// One scenario after lowering and emission.
pub struct ProcessedScenario {
    /// The bound scenario (kept for execution-time re-lowering with the live
    /// World — `${global:key}` resolves at lower time of the scenario).
    pub bound: bind::BoundScenario,
    /// The lowered scenario (batches, secrets, globals, warnings).
    pub lowered: LoweredScenario,
    /// The emitted artifact set (absent when nothing lowers to hurl entries).
    pub artifact: Option<Artifact>,
}

/// The complete front-end result.
pub struct FrontEnd {
    /// Processed features, sorted by path.
    pub features: Vec<LoadedFeature>,
    /// The loaded macro set (shared with execution-time re-lowering).
    pub packs: Arc<PackSet>,
    /// The injected environment snapshot.
    pub env: Arc<BTreeMap<String, String>>,
    /// The injected `proef.toml` config scope (`${url:…}` / `${vars:…}`), with
    /// the active `[env.<name>]` already deep-merged in by the caller.
    pub config_vars: Arc<BTreeMap<String, String>>,
    /// Step kind prefix → engine id.
    pub kind_to_engine: Arc<BTreeMap<String, String>>,
    /// The run id used for this front-end pass.
    pub run_id: Arc<str>,
    /// How many pack sources loaded (builtin + project).
    pub packs_loaded: usize,
    /// Loaded macro count.
    pub macros_loaded: usize,
    /// All soft findings (rendered, but never fatal).
    pub warnings: Vec<Diag>,
}

/// Run the front end over `path` (a `.feature` file or a directory tree).
/// `run_id` overrides the generated uuid-v7 (deterministic artifact hand-off).
// One cohesive listing of the pipeline; splitting hides the stage order.
#[allow(clippy::too_many_lines)]
pub fn run(
    path: &Path,
    mode: ResolveMode,
    run_id: Option<String>,
    config_vars: Arc<BTreeMap<String, String>>,
) -> Result<FrontEnd, FrontError> {
    let engines = registry::engines();
    let kinds: Vec<proef_core::engine::StepKindSpec> = engines
        .iter()
        .flat_map(|e| e.step_kinds().iter().copied())
        .collect();
    let kind_to_engine: BTreeMap<String, String> = engines
        .iter()
        .flat_map(|e| {
            e.step_kinds()
                .iter()
                .map(|k| (k.prefix.to_owned(), e.id().to_owned()))
        })
        .collect();

    // Injected values — the core reads no environment, clock, or randomness.
    // `vars_os`: a foreign non-UTF-8 variable in the environment must not
    // abort the run; it can never match a UTF-8 `${env:…}` reference anyway.
    let env: Arc<BTreeMap<String, String>> = Arc::new(
        std::env::vars_os()
            .filter_map(|(key, value)| Some((key.into_string().ok()?, value.into_string().ok()?)))
            .collect(),
    );
    let run_id: Arc<str> = Arc::from(
        run_id
            .unwrap_or_else(|| uuid::Uuid::now_v7().to_string())
            .as_str(),
    );
    let world = World::new(GlobalStore::load(Path::new(".proef-state.json"))?);

    let mut sources = pack::builtin_sources();
    sources.extend(project_packs(path)?);
    let packs_loaded = sources.len();
    let packs: Arc<PackSet> = Arc::new(pack::load(&sources, &kinds)?);
    let kind_to_engine = Arc::new(kind_to_engine);

    let mut diags: Vec<Diag> = Vec::new();
    let mut warnings: Vec<Diag> = Vec::new();
    let mut features: Vec<LoadedFeature> = Vec::new();

    for feature_path in discover_features(path)? {
        let display = portable_display(&feature_path);
        let text = std::fs::read_to_string(&feature_path).map_err(|err| {
            FrontError::Core(CoreError::system_with(
                format!("cannot read feature file {display}"),
                err,
            ))
        })?;
        let file = match feature::parse(&display, &text) {
            Ok(file) => file,
            Err(errs) => {
                diags.extend(errs);
                continue;
            }
        };
        let bound = match bind::bind(&file, &packs) {
            Ok(bound) => bound,
            Err(errs) => {
                diags.extend(errs);
                continue;
            }
        };
        let ctx = LowerCtx {
            feature: &file,
            packs: &packs,
            kind_to_engine: &kind_to_engine,
            env: &env,
            config_vars: &config_vars,
            run_id: &run_id,
            world: &world,
            mode,
        };
        let feature_stem = feature_path.file_stem().map_or_else(
            || "feature".to_owned(),
            |s| s.to_string_lossy().into_owned(),
        );
        let mut scenarios = Vec::new();
        for scenario in bound {
            match lower::lower(&scenario, &ctx) {
                Ok(lowered) => {
                    warnings.extend(lowered.warnings.iter().cloned());
                    let artifact = emit::emit(&lowered, &feature_stem, &world);
                    if let Some(artifact) = &artifact {
                        validate_artifact(artifact, &lowered, &kinds, &mut diags);
                    }
                    scenarios.push(ProcessedScenario {
                        bound: scenario,
                        lowered,
                        artifact,
                    });
                }
                Err(errs) => diags.extend(errs),
            }
        }
        features.push(LoadedFeature { file, scenarios });
    }

    // Bind/parse warnings travel inside diags; split them out.
    let (errors, softs): (Vec<_>, Vec<_>) = diags
        .into_iter()
        .partition(|d| d.severity == proef_core::diag::Severity::Error);
    warnings.extend(softs);

    if errors.is_empty() {
        Ok(FrontEnd {
            macros_loaded: packs.macros.len(),
            features,
            packs,
            env,
            config_vars,
            kind_to_engine,
            run_id,
            packs_loaded,
            warnings,
        })
    } else {
        let mut all = errors;
        all.extend(warnings);
        Err(FrontError::Diagnostics(all))
    }
}

/// A path as it appears in events, artifacts, and diagnostics: always
/// `/`-separated. The run record and snapshot corpus are a cross-platform
/// contract — a Windows run must not render `suite\case.feature` where every
/// other platform (and the golden corpus) says `suite/case.feature`. Windows
/// APIs accept `/`, so the same string still opens the file.
fn portable_display(path: &Path) -> String {
    let display = path.display().to_string();
    if cfg!(windows) {
        display.replace('\\', "/")
    } else {
        display
    }
}

/// Every `.feature` under `path` (or `path` itself), sorted for determinism.
pub fn discover_features(path: &Path) -> Result<Vec<PathBuf>, FrontError> {
    let mut found = Vec::new();
    if path.is_file() {
        found.push(path.to_path_buf());
    } else if path.is_dir() {
        let mut visited = std::collections::BTreeSet::new();
        walk_features(path, &mut found, &mut visited)?;
    } else {
        return Err(FrontError::Core(CoreError::user(format!(
            "`{}` is neither a feature file nor a directory",
            path.display()
        ))));
    }
    if found.is_empty() {
        return Err(FrontError::Core(CoreError::user(format!(
            "no `.feature` files found under `{}`",
            path.display()
        ))));
    }
    found.sort();
    Ok(found)
}

/// The one recursive directory walk behind feature and pack discovery:
/// symlink-cycle guarded (each cycle copy would become a same-named scenario
/// issuing real traffic). `on_file` receives the containing directory and
/// the file path and applies the caller's predicate.
fn walk_dir(
    dir: &Path,
    visited: &mut std::collections::BTreeSet<PathBuf>,
    on_file: &mut impl FnMut(&Path, PathBuf),
) -> Result<(), FrontError> {
    if let Ok(real) = dir.canonicalize()
        && !visited.insert(real)
    {
        return Ok(());
    }
    let entries = std::fs::read_dir(dir).map_err(|err| {
        FrontError::Core(CoreError::system_with(
            format!("cannot read directory {}", dir.display()),
            err,
        ))
    })?;
    for entry in entries.flatten() {
        let entry_path = entry.path();
        if entry_path.is_dir() {
            walk_dir(&entry_path, visited, on_file)?;
        } else {
            on_file(dir, entry_path);
        }
    }
    Ok(())
}

fn walk_features(
    dir: &Path,
    out: &mut Vec<PathBuf>,
    visited: &mut std::collections::BTreeSet<PathBuf>,
) -> Result<(), FrontError> {
    walk_dir(dir, visited, &mut |_, file| {
        if file.extension().is_some_and(|e| e == "feature") {
            out.push(file);
        }
    })
}

/// Every pack file under `base`: the yaml files of `packs/` directories at
/// any depth — feature discovery recurses, so nested suites must find their
/// packs too — symlink-cycle guarded and sorted for determinism. A `base`
/// that itself is named `packs` contributes its own files.
pub fn pack_files(base: &Path) -> Result<Vec<PathBuf>, FrontError> {
    let mut out = Vec::new();
    let mut visited = std::collections::BTreeSet::new();
    walk_dir(base, &mut visited, &mut |dir, file| {
        if dir.file_name().is_some_and(|name| name == "packs")
            && file.extension().is_some_and(|e| e == "yaml" || e == "yml")
        {
            out.push(file);
        }
    })?;
    out.sort();
    Ok(out)
}

/// Project packs: every `packs/*.yml|yaml` under the input directory (or the
/// input file's parent) via [`pack_files`].
fn project_packs(path: &Path) -> Result<Vec<PackSource>, FrontError> {
    let base = if path.is_dir() {
        path.to_path_buf()
    } else {
        crate::fsutil::parent_dir(path)
    };
    let mut sources = Vec::new();
    for pack_path in pack_files(&base)? {
        let text = std::fs::read_to_string(&pack_path).map_err(|err| {
            FrontError::Core(CoreError::system_with(
                format!("cannot read pack {}", pack_path.display()),
                err,
            ))
        })?;
        sources.push(PackSource {
            name: portable_display(&pack_path),
            text: Arc::from(text.as_str()),
        });
    }
    Ok(sources)
}

/// The shared "filters selected nothing" refusal (exit 2): a typo'd filter
/// must never produce a silent green run.
pub fn no_scenarios_matched() -> proef_core::error::ExitCode {
    crate::render::errln!("error: no scenarios matched the filters (check --tags/--scenario)");
    proef_core::error::ExitCode::UserError
}

/// Does a scenario pass the `--tags` filter? No expression (the flag was
/// omitted) selects everything; otherwise the boolean expression decides.
pub fn tag_selected(tags: &[String], filter: Option<&proef_core::tags::TagExpr>) -> bool {
    filter.is_none_or(|expr| expr.eval(tags))
}
