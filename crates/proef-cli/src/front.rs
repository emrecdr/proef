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
use proef_core::pack::{self, FragmentCorpus, PackSet, PackSource};
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

/// Load just the macro packs for `path` — the vocabulary, without binding any
/// feature to it.
///
/// [`run`] does this as its first stage and then throws the result away if a
/// later stage fails, so a caller that only needs the vocabulary (`macros`
/// listing a suite whose steps do not bind) would otherwise get nothing. The
/// packs are complete whenever this returns: pack loading precedes binding and
/// does not depend on it.
pub fn load_packs(
    path: &Path,
    fragments: &FragmentCorpus,
    naming: &SourceNaming,
) -> Result<PackSet, FrontError> {
    let kinds = registry::step_kinds();
    Ok(load_pack_set(path, fragments, &kinds, naming)?.1)
}

/// Read the configured fragment root into a [`FragmentCorpus`] — once per CLI
/// invocation, whatever it is then used for.
///
/// A `proef test` loads packs up to four times (the suite, then `[run] setup`
/// and `[run] teardown`, each validated and then run). Those loads use
/// different feature paths but always the same corpus, so re-reading and
/// re-scanning it per load was pure repetition; the corpus is read here and
/// scans itself at most once, lazily, on first use.
///
/// Built per invocation rather than cached in a static: `--watch` re-enters
/// `execute` after every edit in the same process, and a corpus that outlived
/// one run would serve the pre-edit corpus to the next.
pub fn fragment_corpus(
    root: Option<&Path>,
    naming: &SourceNaming,
) -> Result<FragmentCorpus, FrontError> {
    let kinds = registry::step_kinds();
    let (sources, read_errors) = match root {
        Some(root) => fragment_sources(root, &kinds, naming)?,
        None => (Vec::new(), Vec::new()),
    };
    Ok(FragmentCorpus::new(sources, &kinds).with_read_errors(read_errors))
}

/// Discover and load a suite's packs — the single place that knows how a
/// suite's vocabulary is assembled. Returns the source count alongside the set,
/// since `run` reports it.
///
/// Both [`load_packs`] and [`run`] go through here: a change to pack discovery
/// (a new source, a caching step) must not be something you can remember in one
/// path and forget in the other.
fn load_pack_set(
    path: &Path,
    fragments: &FragmentCorpus,
    kinds: &[proef_core::engine::StepKindSpec],
    naming: &SourceNaming,
) -> Result<(usize, PackSet), FrontError> {
    let mut sources = pack::builtin_sources();
    sources.extend(project_packs(path, naming)?);
    let packs_loaded = sources.len();
    Ok((packs_loaded, pack::load(&sources, fragments, kinds)?))
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
    fragments: &FragmentCorpus,
    state_file: &Path,
    naming: &SourceNaming,
) -> Result<FrontEnd, FrontError> {
    let kinds = registry::step_kinds();
    let kind_to_engine = registry::kind_to_engine();

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
    let world = World::new(GlobalStore::load(state_file)?);

    let (packs_loaded, loaded) = load_pack_set(path, fragments, &kinds, naming)?;
    let packs: Arc<PackSet> = Arc::new(loaded);
    let kind_to_engine = Arc::new(kind_to_engine);

    let mut diags: Vec<Diag> = Vec::new();
    let mut warnings: Vec<Diag> = Vec::new();
    let mut features: Vec<LoadedFeature> = Vec::new();

    for feature_path in discover_features(path)? {
        let display = naming.name(&feature_path);
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

/// How a discovered path is **spelled** in artifacts, sidecars, events,
/// diagnostics and the console — the one naming boundary for every input kind
/// the front end reads.
///
/// It exists because resolution and naming pull in opposite directions. A path
/// written in `proef.toml` resolves against the config's directory, so it
/// arrives here absolute; ADR-0010 makes the emitted `.hurl` a contract that two
/// machines must reproduce byte-for-byte from identical inputs, and an absolute
/// path breaks that by naming the machine. The rule is therefore the naming dual
/// of [`ProjectConfig::resolve`](crate::config::ProjectConfig): *resolve against
/// the project, then name against the project again.*
///
/// A path that is already relative is left exactly as it arrived — it is
/// machine-independent already, and it is the spelling the caller typed, which
/// their terminal can still open. Only an absolute path is rewritten, and only
/// when it lies inside the project; a corpus genuinely outside the project has
/// no project-relative name and keeps the one it has. This matches how the
/// neighbouring tools anchor a report — pytest builds a nodeid relative to
/// `rootdir`, Jest and Playwright report against `rootDir`.
///
/// Note what this is *not* used for: `DiskSourceProvider` (`proef lsp`) keeps
/// absolute names on purpose, because it keys document identity on them and
/// `documents::name_to_url` refuses a relative name.
// No `Default`: an un-anchored naming is a compile-passing wrong value for a
// call site that skipped `commands::naming`, and nothing legitimate wants one
// — the no-config state goes through `anchored_at(None)`, on purpose, where
// it is visible.
#[derive(Clone)]
pub struct SourceNaming {
    /// The directory holding `proef.toml`, when the project has one. `None` —
    /// no config in scope — means nothing to anchor against, which is also the
    /// state the config-independent reference corpus runs in.
    project: Option<PathBuf>,
    /// The same directory with symlinks resolved, resolved **once** here rather
    /// than per file. See [`Self::relative`] for what it is for.
    canonical: Option<PathBuf>,
}

impl SourceNaming {
    /// Anchor names at `project` (`ProjectConfig::root`).
    #[must_use]
    pub fn anchored_at(project: Option<&Path>) -> Self {
        Self {
            canonical: project.and_then(|project| project.canonicalize().ok()),
            project: project.map(Path::to_path_buf),
        }
    }

    /// The name `path` carries from here on.
    fn name(&self, path: &Path) -> String {
        self.relative(path)
            .map_or_else(|| portable_display(path), |rest| portable_display(&rest))
    }

    /// `path` relative to the project, or `None` when it lies outside.
    ///
    /// Lexically first, which handles the escaping case for free:
    /// `[run] suite = "../shared"` resolves to `<root>/../shared` — `Path::join`
    /// appends without normalizing — so stripping the root returns
    /// `../shared`, still machine-independent. It also keeps the caller's own
    /// spelling when a suite is reached *through* a symlink, which resolving
    /// first would replace with the link's target.
    ///
    /// Canonically second, for the one case the lexical pass cannot see: the
    /// same file named two ways. `proef test /tmp/proj/suite` on macOS is
    /// `/private/tmp/proj/suite` to `current_dir()`, so the prefixes differ by
    /// an alias and the strip fails, leaving the absolute path this type exists
    /// to remove. Comparing canonical forms settles identity rather than
    /// spelling — the same two-step, in the same order, that `watch::same_file`
    /// settled on after a relative `--config` silently matched nothing (R11-9).
    /// Both sides are canonicalized or neither is, so a Windows `\\?\` prefix
    /// appears on both and cancels instead of reaching the name.
    fn relative(&self, path: &Path) -> Option<PathBuf> {
        if let Some(project) = self.project.as_deref()
            && let Ok(rest) = path.strip_prefix(project)
        {
            return Some(rest.to_path_buf());
        }
        // A relative path that did not strip is kept exactly as it arrived —
        // the documented contract, and the common case (every typed-relative
        // suite path reaches here). The guard sits *after* the lexical strip,
        // not before it: `is_relative` is platform-relative — on Windows a
        // rootful, prefix-less `/proj/x` counts as relative — while
        // `strip_prefix` is purely lexical and must keep working on every
        // spelling. What the guard fences off is only the canonicalize
        // fallback below: a syscall chain per discovered file to recompute a
        // spelling the caller already typed — and for a `../` path, to
        // *rewrite* it, against the contract.
        if path.is_relative() {
            return None;
        }
        let canonical = self.canonical.as_deref()?;
        path.canonicalize()
            .ok()?
            .strip_prefix(canonical)
            .ok()
            .map(Path::to_path_buf)
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
/// Directories a suite never lives in, skipped before they are descended.
///
/// The walk is unindexed and runs per LSP request, so the cost of entering one
/// of these is paid over and over for a subtree that cannot contain a `.feature`
/// or a `packs/` worth finding. `target/` alone can hold hundreds of thousands
/// of files.
const SKIPPED_DIRS: &[&str] = &["target", "node_modules", "vendor"];

/// How deep the walk will recurse before refusing.
///
/// Matches the `use:` nesting bound (ADR-0004) rather than inventing a second
/// number. A suite nested deeper than this is pathological, and without a bound
/// a symlink arrangement the `visited` guard cannot see — or simply a very deep
/// tree — recurses until the stack runs out.
const MAX_WALK_DEPTH: usize = 32;

fn walk_dir(
    dir: &Path,
    visited: &mut std::collections::BTreeSet<PathBuf>,
    on_file: &mut impl FnMut(&Path, PathBuf),
) -> Result<(), FrontError> {
    walk_dir_inner(dir, visited, on_file, 0)
}

fn walk_dir_inner(
    dir: &Path,
    visited: &mut std::collections::BTreeSet<PathBuf>,
    on_file: &mut impl FnMut(&Path, PathBuf),
    depth: usize,
) -> Result<(), FrontError> {
    if depth > MAX_WALK_DEPTH {
        return Err(FrontError::Core(CoreError::user(format!(
            "directory nesting deeper than {MAX_WALK_DEPTH} under {} — \
             narrow the suite path or flatten the tree",
            dir.display()
        ))));
    }
    if let Ok(real) = dir.canonicalize()
        && !visited.insert(real)
    {
        return Ok(());
    }
    // An unreadable *root* is the caller's own path and must be loud. An
    // unreadable descendant is somebody else's directory that happens to sit
    // under it — one `Permission denied` deep in a tree used to abort the whole
    // walk, and the LSP then swallowed that error into an empty analysis, so a
    // single unreadable subdirectory silently emptied the suite. Skip it and
    // keep going, the way `find` and ripgrep do.
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) if depth > 0 => return Ok(()),
        Err(err) => {
            return Err(FrontError::Core(CoreError::system_with(
                format!("cannot read directory {}", dir.display()),
                err,
            )));
        }
    };
    for entry in entries.flatten() {
        let entry_path = entry.path();
        if entry_path.is_dir() {
            // Tested on the child, never the root: a suite may legitimately be
            // rooted *at* a hidden directory, and refusing to walk the path the
            // caller explicitly named would be the tool second-guessing them.
            if skipped_dir(&entry_path) {
                continue;
            }
            walk_dir_inner(&entry_path, visited, on_file, depth + 1)?;
        } else {
            on_file(dir, entry_path);
        }
    }
    Ok(())
}

/// Whether a *child* directory should not be descended: build output, vendored
/// dependencies, and dot-directories (`.git`, `.proef-runs`, `.venv`, …).
pub(crate) fn skipped_dir(path: &Path) -> bool {
    path.file_name().is_some_and(|name| {
        let name = name.to_string_lossy();
        name.starts_with('.') || SKIPPED_DIRS.contains(&name.as_ref())
    })
}

fn walk_features(
    dir: &Path,
    out: &mut Vec<PathBuf>,
    visited: &mut std::collections::BTreeSet<PathBuf>,
) -> Result<(), FrontError> {
    walk_dir(dir, visited, &mut |_, file| {
        if file.extension().is_some_and(|e| e == FEATURE_EXT) {
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
            && file
                .extension()
                .is_some_and(|e| PACK_EXTS.iter().any(|ext| e == *ext))
        {
            out.push(file);
        }
    })?;
    out.sort();
    Ok(out)
}

/// The extension of a feature file.
pub(crate) const FEATURE_EXT: &str = "feature";

/// The extensions of a pack file.
pub(crate) const PACK_EXTS: [&str; 2] = ["yaml", "yml"];

/// Every extension proef itself authors — the counterpart to
/// [`fragment_extensions`], which answers the same question for the engines.
///
/// One definition because four places need the same answer: feature discovery,
/// pack discovery, `fmt`'s explicit-file predicate, and `--watch`'s retrigger
/// allowlist. `--watch` used to hardcode its own copy two lines below a comment
/// explaining why the *engine* half must never be hardcoded; teaching discovery
/// a new extension would have left the watcher silently not retriggering on it.
pub(crate) fn authored_extensions() -> Vec<&'static str> {
    let mut exts = vec![FEATURE_EXT];
    exts.extend(PACK_EXTS);
    exts
}

/// The file extensions the registered engines claim for fragment files. The one
/// place that answers "is this a fragment?", so discovery, `--watch` and any
/// future consumer cannot disagree — a second engine's format must not be
/// something one of them knows about and another does not (ADR-0002).
pub fn fragment_extensions(kinds: &[proef_core::engine::StepKindSpec]) -> Vec<&'static str> {
    kinds
        .iter()
        .filter_map(|kind| Some(kind.fragments?.ext))
        .collect()
}

/// Every fragment file under `root`, recursively (ADR-0018). Unlike packs
/// there is no directory convention: the root is named in `[run] fragments`,
/// so everything beneath it with a claimed extension is in scope. The
/// extensions come from the registered engines' `fragments.ext`, so discovery never
/// learns a file type of its own (ADR-0002).
pub fn fragment_files(
    root: &Path,
    kinds: &[proef_core::engine::StepKindSpec],
) -> Result<Vec<PathBuf>, FrontError> {
    let supports: Vec<proef_core::engine::FragmentSupport> =
        kinds.iter().filter_map(|kind| kind.fragments).collect();
    if supports.is_empty() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    let mut visited = std::collections::BTreeSet::new();
    walk_dir(root, &mut visited, &mut |_, file| {
        // Through the seam's own predicate, so discovery and the scan cannot
        // disagree about which files are fragments.
        if supports
            .iter()
            .any(|support| support.claims(&file.to_string_lossy()))
        {
            out.push(file);
        }
    })?;
    out.sort();
    Ok(out)
}

/// Read every fragment file under the configured root into [`PackSource`]s.
/// A missing root is a user error, not a silent empty set: it was named on
/// purpose, so a typo must not read as "this suite has no fragments".
pub fn fragment_sources(
    root: &Path,
    kinds: &[proef_core::engine::StepKindSpec],
    naming: &SourceNaming,
) -> Result<(Vec<PackSource>, Vec<Diag>), FrontError> {
    if !root.exists() {
        return Err(FrontError::Core(CoreError::user(format!(
            "`[run] fragments` names `{}`, which does not exist",
            root.display()
        ))));
    }
    Ok(read_corpus(fragment_files(root, kinds)?, naming))
}

/// Project packs: every `packs/*.yml|yaml` under the input directory (or the
/// input file's parent) via [`pack_files`].
fn project_packs(path: &Path, naming: &SourceNaming) -> Result<Vec<PackSource>, FrontError> {
    let base = if path.is_dir() {
        path.to_path_buf()
    } else {
        crate::fsutil::parent_dir(path)
    };
    read_sources(pack_files(&base)?, naming)
}

/// Read discovered packs into [`PackSource`]s, failing on the first unreadable
/// one.
///
/// Packs only. The other input kind the front end loads is a fragment corpus,
/// which [`read_corpus`] handles instead — it collects per-file rather than
/// failing, because a corpus is foreign by design. That difference is why the
/// two are separate functions rather than one with a flag.
fn read_sources(paths: Vec<PathBuf>, naming: &SourceNaming) -> Result<Vec<PackSource>, FrontError> {
    let mut sources = Vec::with_capacity(paths.len());
    for path in paths {
        let text = std::fs::read_to_string(&path).map_err(|err| {
            FrontError::Core(CoreError::system_with(
                format!("cannot read pack {}", path.display()),
                err,
            ))
        })?;
        sources.push(PackSource {
            name: naming.name(&path),
            text: Arc::from(text.as_str()),
        });
    }
    Ok(sources)
}

/// Read what can be read, and shape the rest as diagnostics.
///
/// Packs are proef's own inputs, so an unreadable one is a hard error
/// ([`read_sources`]). A **fragment corpus is foreign by design** — `CONFIG.md`
/// sells "pointing at a corpus you did not write costs nothing" — and one
/// binary or latin-1 file in it used to exit 3 from every command, including
/// `flows`, which never looks at a fragment. Pack loading and the annotation
/// scan both already collect per-file and never sink their siblings; this was
/// the one stage that still did.
/// **Bounded**, which the resilience work did not make it. A corpus is foreign
/// by design and named by one config line, so its size is not something the
/// suite's author controls: `[run] fragments` pointed one directory too high
/// reads whatever is under it. A 279 MB file cost 601 MB of resident memory on
/// `proef flows` — a command that never looks at a fragment — because the text
/// is read whole and then copied into an `Arc<str>`. The size is checked from
/// the directory entry, so an oversized file is never allocated at all.
fn read_corpus(paths: Vec<PathBuf>, naming: &SourceNaming) -> (Vec<PackSource>, Vec<Diag>) {
    use proef_core::pack::{Admit, CorpusBudget};

    let mut sources = Vec::with_capacity(paths.len());
    let mut errors = Vec::new();
    // The decision (skip-without-charging, stop-not-skip on exhaustion) lives
    // in core with the constants it binds; only the *measurement* is ours —
    // the directory entry, so an oversized file is never allocated at all.
    let mut budget = CorpusBudget::new();
    for path in paths {
        let name = naming.name(&path);
        // An unstattable file is left to the read below to report: it fails
        // there with the real cause, and reporting "cannot size" first would
        // explain the same file twice in different words.
        let size = std::fs::metadata(&path).map_or(0, |meta| meta.len());
        match budget.admit(&name, size) {
            Admit::Skip(diag) => {
                errors.push(diag);
                continue;
            }
            Admit::Stop(diag) => {
                errors.push(diag);
                break;
            }
            Admit::Read => {}
        }
        match std::fs::read_to_string(&path) {
            Ok(text) => sources.push(PackSource {
                name,
                text: Arc::from(text.as_str()),
            }),
            Err(err) => errors.push(proef_core::pack::FragmentCorpus::unreadable_file(
                &name,
                &err.to_string(),
            )),
        }
    }
    (sources, errors)
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

/// Warn when `[run] exclusive-tags` is set and matches no scenario in the suite.
///
/// The key exists because isolation must never degrade silently: that is the
/// stated reason it is a config expression rather than a reserved tag name. A
/// typo defeats it exactly the way the tag form would — `@soloz` against a
/// `@solo` suite put every scenario back in the shared pool, exit 0, nothing
/// said — and the resulting interference reads as flakiness rather than as a
/// misspelled setting.
///
/// Judged over **every** scenario the suite loaded, not over the ones this run
/// selected, so `--tags @smoke` filtering the exclusive scenarios out of one run
/// is not reported as a broken setting. A warning rather than an error: the
/// expression is well-formed, and a suite mid-refactor may legitimately have
/// none of them left.
pub fn warn_if_exclusive_matches_nothing(
    front: &FrontEnd,
    exclusive: Option<&proef_core::tags::TagExpr>,
    written: Option<&str>,
) {
    let (Some(expr), Some(written)) = (exclusive, written) else {
        return;
    };
    let matches = front
        .features
        .iter()
        .flat_map(|feature| feature.scenarios.iter())
        .any(|scenario| expr.eval(&scenario.lowered.tags));
    if matches {
        return;
    }
    crate::render::errln!(
        "warning: [run] exclusive-tags (`{written}`) matches no scenario — every scenario \
         is running in the shared pool\n         \
         check the spelling against `proef flows`, which prints each scenario's tags"
    );
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::{SourceNaming, discover_features, pack_files};
    use std::path::{Path, PathBuf};

    /// The naming rule, case by case. Each line is a spelling that reaches a
    /// durable artifact, so each is a contract rather than an implementation
    /// detail (ADR-0010).
    #[test]
    fn a_name_is_never_absolute_while_the_file_is_inside_the_project() {
        let project = PathBuf::from("/proj");
        let naming = SourceNaming::anchored_at(Some(&project));

        // The regression this fixes: `[run] suite` resolves against the config,
        // so the path arrives absolute and used to be recorded that way.
        assert_eq!(
            naming.name(Path::new("/proj/suite/health.feature")),
            "suite/health.feature"
        );

        // A relative path is machine-independent already, and it is the
        // spelling the caller typed — their terminal can still open it.
        assert_eq!(
            naming.name(Path::new("suite/health.feature")),
            "suite/health.feature"
        );

        // `[run] suite = "../shared"` resolves to `<root>/../shared` because
        // `Path::join` appends without normalizing, so stripping the root
        // leaves a `..` path — still machine-independent, which is the point.
        assert_eq!(
            naming.name(Path::new("/proj/../shared/x.feature")),
            "../shared/x.feature"
        );

        // A corpus genuinely outside the project has no project-relative name,
        // so it keeps the only one it has rather than being mangled.
        assert_eq!(
            naming.name(Path::new("/opt/corpus/x.hurl")),
            "/opt/corpus/x.hurl"
        );
    }

    /// No `proef.toml` in scope means nothing to anchor against — the state the
    /// config-independent reference corpus runs in, where every path is already
    /// relative and must stay exactly as it arrived.
    #[test]
    fn without_a_project_every_name_is_left_as_it_arrived() {
        let naming = SourceNaming::anchored_at(None);
        assert_eq!(
            naming.name(Path::new("tests/features/a.feature")),
            "tests/features/a.feature"
        );
        assert_eq!(naming.name(Path::new("/abs/a.feature")), "/abs/a.feature");
    }

    /// Build output, vendored trees and dot-directories are never descended.
    /// The walk is unindexed and re-runs per LSP request, so entering `target/`
    /// costs that price repeatedly for a subtree that cannot hold a suite.
    #[test]
    fn the_walk_skips_build_output_and_dot_directories() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        for dir in ["suite", "target/debug", "node_modules/pkg", ".git/objects"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        std::fs::write(root.join("suite/real.feature"), "Feature: F\n").unwrap();
        for buried in [
            "target/debug/x.feature",
            "node_modules/pkg/y.feature",
            ".git/objects/z.feature",
        ] {
            std::fs::write(root.join(buried), "Feature: F\n").unwrap();
        }

        let found = discover_features(root).unwrap();
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].ends_with("real.feature"), "{found:?}");
    }

    /// A suite may legitimately be rooted *at* an excluded name — the exclusion
    /// is about what lies beneath a root, not about refusing the path the
    /// caller explicitly named.
    #[test]
    fn a_root_that_is_itself_an_excluded_name_is_still_walked() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join(".hidden-suite");
        std::fs::create_dir_all(root.join("packs")).unwrap();
        std::fs::write(root.join("case.feature"), "Feature: F\n").unwrap();
        std::fs::write(root.join("packs/p.yaml"), "macros: {}\n").unwrap();

        assert_eq!(discover_features(&root).unwrap().len(), 1);
        assert_eq!(pack_files(&root).unwrap().len(), 1);
    }

    /// One unreadable directory deep in a tree used to abort the entire walk —
    /// and the LSP swallowed that error into an empty analysis, so a single
    /// permission-denied subdirectory silently emptied the suite.
    #[cfg(unix)]
    #[test]
    fn an_unreadable_subdirectory_does_not_empty_the_suite() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("readable")).unwrap();
        std::fs::create_dir_all(root.join("locked")).unwrap();
        std::fs::write(root.join("readable/real.feature"), "Feature: F\n").unwrap();
        std::fs::set_permissions(root.join("locked"), std::fs::Permissions::from_mode(0o000))
            .unwrap();

        let found = discover_features(root);
        // Restore before asserting, so a failure cannot leave an undeletable dir.
        std::fs::set_permissions(root.join("locked"), std::fs::Permissions::from_mode(0o755))
            .unwrap();

        let found = found.expect("an unreadable subdirectory must not fail the walk");
        assert_eq!(found.len(), 1, "{found:?}");
    }
}
