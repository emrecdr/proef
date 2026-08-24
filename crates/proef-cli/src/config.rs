//! Project configuration: `proef.toml` (TECH-SPEC §10 precedence — defaults <
//! `proef.toml` < active environment (`[env.<name>]`) < flags). No general
//! environment-variable layer exists over these keys — `PROEF_ENV` selects the
//! profile and `PROEF_SECRET_*`/`PROEF_KEY`/`PROEF_CONFIG_DIR` are their own
//! channels, none of them an override of a `proef.toml` value (R17 deep audit:
//! this comment claimed a fifth layer the code never had).
//!
//! Layout mirrors the base tables under each environment (the Wrangler/Cargo-profile
//! model): `[url]`/`[vars]`/`[http]` are the base sections and
//! `[env.<name>.url]`/`.vars`/`.http` deep-merge over them when `--env <name>`
//! (or `PROEF_ENV`) selects that environment. `[env.<name>.run]` overrides only
//! `jobs` — `runs-dir` and `suite` are project-wide (one record store, one default
//! suite), so listing them under an environment is a hard error, never a silent
//! no-op. `url` and `vars` values reach packs as `${url:<key>}` / `${vars:<key>}`;
//! secrets stay on their own encrypted channel and never appear here.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use proef_core::engine::HttpDefaults;
use serde::Deserialize;

use crate::sla::SlaThresholds;

/// The nearest `proef.toml` walking up from the working directory (like
/// cargo/git), or `None` if none exists in any ancestor.
fn find_config() -> Option<PathBuf> {
    find_config_from(&std::env::current_dir().ok()?)
}

/// The nearest `proef.toml` at or above `dir` (cargo/git style).
///
/// Split out from [`find_config`] because `proef lsp` must search from the
/// workspace root the *client* announced, not from wherever the editor happened
/// to be launched — a server started in `$HOME` would otherwise adopt `$HOME`'s
/// config, or none, for a project two directories down.
fn find_config_from(dir: &Path) -> Option<PathBuf> {
    dir.ancestors()
        .map(|dir| dir.join("proef.toml"))
        .find(|candidate| candidate.is_file())
}

/// Loaded `proef.toml` (all fields optional — defaults apply).
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectConfig {
    /// The `proef.toml` this was read from, when there was one.
    ///
    /// The *file*, not its directory: `root` is derived from it. Keeping the
    /// directory alone meant every consumer that needed the file — `--watch`
    /// deciding what to watch, `proef lsp` deciding what to load — had to
    /// rediscover it by walking up from the working directory, which silently
    /// ignored `--config` and left the editor analysing a different project
    /// than the runner ran.
    #[serde(skip)]
    path: Option<PathBuf>,
    /// `[run]` table.
    #[serde(default)]
    pub run: RunTable,
    /// `[http]` table.
    #[serde(default)]
    pub http: HttpTable,
    /// `[meta]` table — static run metadata (team, component); merged
    /// under the active `[env.<name>.meta]`, then under `--meta` flags
    /// (ADR-0020). Values proef records verbatim and never interprets.
    #[serde(default)]
    pub meta: BTreeMap<String, String>,
    /// `[sla]` table — the opt-in run-level latency budget.
    #[serde(default)]
    pub sla: SlaTable,
    /// `[url]` table — URL variables (`${url:base}`, `${url:admin}`, …).
    #[serde(default)]
    pub url: BTreeMap<String, String>,
    /// `[vars]` table — non-secret variables (`${vars:apiVersion}`, …).
    #[serde(default)]
    pub vars: BTreeMap<String, String>,
    /// `[env.<name>]` profiles that override the base sections.
    #[serde(default)]
    pub env: BTreeMap<String, EnvProfile>,
}

/// `[run]` settings (env-overridable via `[env.<name>.run]`).
#[derive(Debug, Default, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct RunTable {
    /// Parallel scenario workers (default: available parallelism).
    pub jobs: Option<usize>,
    /// Run-record directory (default `.proef-runs`).
    #[serde(rename = "runs-dir")]
    pub runs_dir: Option<String>,
    /// How many past run records to keep in `runs-dir`
    /// (default [`exec::DEFAULT_KEEP_RUNS`](crate::exec::DEFAULT_KEEP_RUNS)).
    ///
    /// `0` keeps none but the run in flight, which is the setting for a suite
    /// re-run on every save; the default suits an archive. Rotation only ever
    /// deletes directories named by a generated run id, so a record written
    /// under `--run-id <name>` is outside this budget entirely.
    #[serde(rename = "keep-runs")]
    pub keep_runs: Option<usize>,
    /// Default suite path used when `proef test` is given no path
    /// (falls back to the `tests/` convention when unset — see `suite`).
    pub suite: Option<String>,
    /// Root directory holding the engine-native **fragment** files a pack may
    /// `ref:` (ADR-0018), scanned recursively. Unset means no fragments — the
    /// feature is opt-in, and naming the root explicitly is what makes reading
    /// a corpus outside the suite a declared act rather than a silent walk.
    pub fragments: Option<String>,
    /// Feature file run **once before** the suite pool (suite-level setup,
    /// ADR-0014). Its `saveAs: global` promotions are visible to every
    /// scenario; a setup failure aborts the run before the pool launches.
    pub setup: Option<String>,
    /// Feature file run **once after** the pool (suite-level teardown,
    /// ADR-0014) — only when setup succeeded. A teardown failure is a distinct
    /// non-zero signal, never a silently-green suite.
    pub teardown: Option<String>,
    /// Tag expression selecting scenarios that run with the pool to themselves.
    ///
    /// A config key rather than a reserved tag name, because the failure mode of
    /// the tag form is the worst kind: a scenario added months later lands
    /// untagged in the parallel pool and breaks isolation intermittently, which
    /// reads as flakiness rather than as a missing declaration. Naming the
    /// expression here keeps the rule in one reviewable place, and it is an
    /// ordinary tag expression (`"@serial and not @wip"`), not a new grammar.
    #[serde(rename = "exclusive-tags")]
    pub exclusive_tags: Option<String>,
}

/// `[env.<name>.run]` overrides. Deliberately narrower than [`RunTable`]: only
/// `jobs` is environment-scoped. `runs-dir` and `suite` are project-wide, so
/// `deny_unknown_fields` rejects them here — a per-environment `suite`/`runs-dir`
/// is a loud error, not the silent no-op that reusing the full `RunTable` caused.
#[derive(Debug, Default, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct RunOverride {
    /// Parallel scenario workers for this environment (overrides `[run] jobs`).
    pub jobs: Option<usize>,
}

/// `[http]` settings (batch-level defaults; per-entry `[Options]` override).
/// Env-overridable via `[env.<name>.http]`.
#[derive(Debug, Default, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct HttpTable {
    /// Per-request timeout in milliseconds.
    #[serde(rename = "timeout-ms")]
    pub timeout_ms: Option<u64>,
    /// Follow redirects.
    #[serde(rename = "follow-location")]
    pub follow_location: Option<bool>,
}

/// `[sla]` settings — an opt-in run-level latency budget (ceilings in
/// milliseconds). Env-overridable via `[env.<name>.sla]`. Every field is
/// optional and unset means "no gate for that metric", so an absent `[sla]`
/// table leaves the run byte-identical to an SLA-unaware build.
#[derive(Debug, Default, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct SlaTable {
    /// 95th-percentile per-step duration ceiling.
    #[serde(rename = "p95-ms")]
    pub p95_ms: Option<u64>,
    /// Maximum single-step duration ceiling.
    #[serde(rename = "max-ms")]
    pub max_ms: Option<u64>,
}

/// An `[env.<name>]` profile: per-environment overrides that deep-merge over the
/// base sections, key by key. Only the deltas need listing; unlisted keys inherit.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvProfile {
    /// `[env.<name>.url]` — URL overrides.
    #[serde(default)]
    pub url: BTreeMap<String, String>,
    /// `[env.<name>.vars]` — variable overrides.
    #[serde(default)]
    pub vars: BTreeMap<String, String>,
    /// `[env.<name>.meta]` — metadata overrides (deep-merged over `[meta]`).
    #[serde(default)]
    pub meta: BTreeMap<String, String>,
    /// `[env.<name>.http]` — HTTP setting overrides.
    #[serde(default)]
    pub http: HttpTable,
    /// `[env.<name>.run]` — run setting overrides (`jobs` only; see [`RunOverride`]).
    #[serde(default)]
    pub run: RunOverride,
    /// `[env.<name>.sla]` — SLA ceiling overrides (looser staging, tighter prod).
    #[serde(default)]
    pub sla: SlaTable,
}

impl ProjectConfig {
    /// Load the nearest `proef.toml`, searching up from the working directory
    /// (like cargo/git) so config is found from any subdirectory. Absent file =
    /// defaults; a malformed file is a user error worth failing loudly on.
    pub fn load() -> Result<Self, String> {
        Self::from_nearest(find_config())
    }

    /// Like [`load`](Self::load), but searching from `dir` instead of the
    /// process working directory — what `proef lsp` needs once the client has
    /// told it where the workspace actually is.
    pub fn load_from(dir: &Path) -> Result<Self, String> {
        Self::from_nearest(find_config_from(dir))
    }

    /// The config file named by `--config`, bypassing discovery entirely.
    ///
    /// Discovery only searches *up*, so a config beside the suite is invisible
    /// from the repository root — a layout an adopting team planned and had to
    /// abandon. Naming the file removes that constraint; every path written
    /// inside it still resolves against its own directory, the one rule.
    ///
    /// A named file that is not there is a **hard error**, unlike discovery
    /// finding nothing. Falling back to defaults would answer a typo'd path
    /// with a run that silently has no configuration — every `${url:…}` unset,
    /// and nothing saying why.
    ///
    /// The path is made **absolute** before it is stored, because everything
    /// downstream already assumes it is. Discovery gets that for free — it
    /// builds on `current_dir()`, which the OS returns already resolved — so
    /// the typed flag is the only way a relative one enters, and it silently
    /// broke two things when it did: [`fragments`](Self::fragments) returned a
    /// relative path, which `documents::name_to_url` refuses, costing
    /// `proef lsp` go-to-definition over the whole corpus; and `--watch`
    /// identifies the config it watches by path, so edits to a relatively-named
    /// one never retriggered a run.
    ///
    /// Lexical (`std::path::absolute`), not `canonicalize`: this answers *where
    /// does it point*, and resolving symlinks here would rewrite a spelling the
    /// user chose into one they never typed. Whether two paths *are the same
    /// file* is a different question, asked and answered where it matters
    /// ([`crate::watch`]).
    pub fn load_at(path: &Path) -> Result<Self, String> {
        if !path.is_file() {
            return Err(format!(
                "--config {} is not a file — name the proef.toml to read, \
                 or omit the flag to search up from the working directory",
                path.display()
            ));
        }
        let absolute = std::path::absolute(path)
            .map_err(|err| format!("--config {} cannot be resolved: {err}", path.display()))?;
        Self::from_nearest(Some(absolute))
    }

    fn from_nearest(found: Option<PathBuf>) -> Result<Self, String> {
        let Some(path) = found else {
            return Ok(Self::default());
        };
        let text = std::fs::read_to_string(&path)
            .map_err(|err| format!("cannot read {}: {err}", path.display()))?;
        let mut config: Self =
            toml::from_str(&text).map_err(|err| format!("{} is invalid: {err}", path.display()))?;
        config.path = Some(path);
        Ok(config)
    }

    /// The active `[env.<name>]` profile, or `None` when no environment is
    /// selected. A named-but-absent environment is a user error (typo) — list
    /// the known names so the fix is obvious.
    fn env_profile(&self, active_env: Option<&str>) -> Result<Option<&EnvProfile>, String> {
        match active_env {
            None => Ok(None),
            Some(name) => self.env.get(name).map(Some).ok_or_else(|| {
                let mut known: Vec<&str> = self.env.keys().map(String::as_str).collect();
                known.sort_unstable();
                if known.is_empty() {
                    format!("unknown environment `{name}` (proef.toml defines no `[env.*]`)")
                } else {
                    format!("unknown environment `{name}` (known: {})", known.join(", "))
                }
            }),
        }
    }

    /// The effective HTTP defaults: builtin < `[http]` < `[env.<name>.http]`,
    /// merged field by field so an environment overrides only what it lists.
    pub fn http_defaults(&self, active_env: Option<&str>) -> Result<HttpDefaults, String> {
        let base = HttpDefaults::default();
        let env_http = self.env_profile(active_env)?.map(|profile| &profile.http);
        Ok(HttpDefaults {
            timeout_ms: env_http
                .and_then(|http| http.timeout_ms)
                .or(self.http.timeout_ms)
                .unwrap_or(base.timeout_ms),
            follow_location: env_http
                .and_then(|http| http.follow_location)
                .or(self.http.follow_location)
                .unwrap_or(base.follow_location),
        })
    }

    /// The effective SLA thresholds: `[sla]` < `[env.<name>.sla]`, field by
    /// field, so an environment overrides only the ceilings it lists. All-`None`
    /// (the default when no `[sla]` table exists) is the inert, no-gate state.
    pub fn sla_thresholds(&self, active_env: Option<&str>) -> Result<SlaThresholds, String> {
        let env_sla = self.env_profile(active_env)?.map(|profile| &profile.sla);
        Ok(SlaThresholds {
            p95_ms: env_sla.and_then(|sla| sla.p95_ms).or(self.sla.p95_ms),
            max_ms: env_sla.and_then(|sla| sla.max_ms).or(self.sla.max_ms),
        })
    }

    /// The effective job count (flag > `[env.<name>.run]` > `[run]` > available
    /// parallelism).
    pub fn jobs(&self, flag: Option<usize>, active_env: Option<&str>) -> Result<usize, String> {
        let env_jobs = self
            .env_profile(active_env)?
            .and_then(|profile| profile.run.jobs);
        Ok(flag
            .or(env_jobs)
            .or(self.run.jobs)
            .unwrap_or_else(|| {
                std::thread::available_parallelism().map_or(4, std::num::NonZero::get)
            })
            .max(1))
    }

    /// A path **written in `proef.toml`**, resolved against the file's own
    /// directory.
    ///
    /// The whole rule, in one function: *paths written in the config resolve
    /// against the config file; paths typed on the command line resolve against
    /// the working directory.* It is the convention every neighbouring tool
    /// follows — a Cargo manifest's `members`/`path` are manifest-relative, a
    /// `tsconfig.json`'s paths are config-relative, pytest makes the config
    /// file's directory the rootdir — and proef followed it for exactly one key,
    /// `[run] fragments`, while `suite`, `setup`, `teardown` and `runs-dir` read
    /// against the working directory. Two keys in one table then meant two
    /// different roots: from a subdirectory, `fragments = "hurl"` resolved and
    /// `suite = "features"` did not exist.
    ///
    /// An absolute path is taken as written — the corpus, or the record store,
    /// may sit outside the project entirely. No `proef.toml` in scope leaves the
    /// path relative, which is the working directory: there is no config file to
    /// be relative *to*, and the config-independent reference corpus depends on
    /// that staying true.
    fn resolve(&self, written: &str) -> PathBuf {
        let raw = PathBuf::from(written);
        if raw.is_absolute() {
            return raw;
        }
        match self.root() {
            Some(root) => root.join(raw),
            None => raw,
        }
    }

    /// The run-record directory (`[run] runs-dir`, default `.proef-runs`).
    /// Not environment-scoped — one project shares one record store, and it is
    /// the *project's*: reading `explain` from a subdirectory must find the
    /// records the run wrote, not an empty directory beside the shell.
    pub fn runs_dir(&self) -> PathBuf {
        self.resolve(self.run.runs_dir.as_deref().unwrap_or(".proef-runs"))
    }

    /// How many past run records `runs-dir` retains (`[run] keep-runs`).
    ///
    /// Project-wide for the same reason `runs-dir` is: one project, one record
    /// store, so one retention policy. `[env.<name>.run]` accepts only `jobs`
    /// and `deny_unknown_fields` refuses this key there — a per-environment
    /// retention would be a silent no-op on a shared directory.
    pub fn keep_runs(&self) -> usize {
        self.run.keep_runs.unwrap_or(crate::exec::DEFAULT_KEEP_RUNS)
    }

    /// The persistent World (`.proef-state.json`, ADR-0005) — one project, one
    /// store. Anchored on the config so two working directories cannot silently
    /// become two Worlds with the same `saveAs: global` writing to each.
    pub fn state_file(&self) -> PathBuf {
        self.resolve(crate::exec::STATE_FILE)
    }

    /// The encrypted secret store (`.proef-secrets.json`) — one project, one
    /// store, for the reason [`state_file`](Self::state_file) gives. The *key*
    /// is unaffected: it lives under `PROEF_CONFIG_DIR`/HOME and is per-machine,
    /// not per-project.
    pub fn secrets_file(&self) -> PathBuf {
        self.resolve(crate::secretstore::STORE_FILE)
    }

    /// The configured fragment root (`[run] fragments`) resolved against the
    /// directory holding `proef.toml`. There is no convention fallback: a
    /// `ref:` with nothing configured reports `unknown_ref` and says so, which
    /// beats guessing at a directory.
    /// Returns a **resolvable** path, never a display spelling. `proef lsp`
    /// feeds this straight to `DiskSourceProvider`, which keys document
    /// identity on absolute source names (`documents::name_to_url` yields
    /// `None` for anything relative), so shortening it here silently costs
    /// go-to-definition and `.hurl`-positioned diagnostics. How a fragment
    /// path is *spelled* in artifacts is a naming question and lives at the
    /// naming boundary — `front::fragment_sources`.
    pub fn fragments(&self) -> Option<PathBuf> {
        Some(self.resolve(self.run.fragments.as_deref()?))
    }

    /// The default suite directory: `[run] suite` if set, else the `tests/`
    /// convention when it exists on disk, else None. The single place both the
    /// suite commands (`resolve_suite_path`) and the LSP derive "which directory
    /// is the suite", so the two never diverge.
    ///
    /// The convention probe is resolved too: `tests/` means the project's, and
    /// probing the working directory answered "no suite" one directory into the
    /// project it was standing in.
    pub fn default_suite_path(&self) -> Option<PathBuf> {
        if let Some(suite) = self.run.suite.as_deref() {
            return Some(self.resolve(suite));
        }
        let convention = self.resolve("tests");
        convention.is_dir().then_some(convention)
    }

    /// The `proef.toml` this config was read from, if any — so a consumer that
    /// needs the *file* (what `--watch` watches, what `proef lsp` loads) uses
    /// the one actually in force rather than searching again.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// The directory relative config paths resolve against: the config file's
    /// own, since it may sit above the working directory.
    ///
    /// Public because it anchors the *naming* half of the same rule as well as
    /// the resolution half. A path written in the config resolves against this
    /// directory, so it comes back absolute — and `front::SourceNaming` spells
    /// it relative to this same directory again before it reaches an artifact
    /// or the run record. One anchor, both directions.
    pub fn root(&self) -> Option<&Path> {
        self.path.as_deref().and_then(Path::parent)
    }

    /// The tag expression selecting exclusive scenarios (`[run] exclusive-tags`),
    /// parsed. A malformed expression is a user error, not a silently-ignored
    /// key: the whole point of the setting is that a scenario it should have
    /// matched must never quietly rejoin the pool.
    pub fn exclusive_tags(&self) -> Result<Option<proef_core::tags::TagExpr>, String> {
        let Some(raw) = self.run.exclusive_tags.as_deref() else {
            return Ok(None);
        };
        proef_core::tags::parse(raw)
            .map(Some)
            .map_err(|err| format!("[run] exclusive-tags is not a valid tag expression: {err}"))
    }

    /// The suite-level setup feature (`[run] setup`), if any (ADR-0014).
    pub fn setup(&self) -> Option<PathBuf> {
        Some(self.resolve(self.run.setup.as_deref()?))
    }

    /// The suite-level teardown feature (`[run] teardown`), if any (ADR-0014).
    pub fn teardown(&self) -> Option<PathBuf> {
        Some(self.resolve(self.run.teardown.as_deref()?))
    }

    /// The injected `${url:…}` / `${vars:…}` scope for the active environment,
    /// keyed `"<namespace>:<key>"` (deep-merged: base then env override). Passed
    /// into `LowerCtx::config_vars` so the sans-IO core resolves these without
    /// The run's metadata: `[meta]` under the active `[env.<name>.meta]` under
    /// the `--meta` flags — the same base < env < flags chain as every other
    /// setting (ADR-0020). Flag pairs arrive pre-parsed and pre-checked (a
    /// duplicate flag key is exit 2 at the parse, loud over last-wins).
    pub fn metadata(
        &self,
        active_env: Option<&str>,
        flag_pairs: &BTreeMap<String, String>,
    ) -> BTreeMap<String, String> {
        let mut merged = self.meta.clone();
        if let Some(profile) = active_env.and_then(|name| self.env.get(name)) {
            for (key, value) in &profile.meta {
                merged.insert(key.clone(), value.clone());
            }
        }
        for (key, value) in flag_pairs {
            merged.insert(key.clone(), value.clone());
        }
        merged
    }

    /// reading any file itself.
    pub fn config_vars(
        &self,
        active_env: Option<&str>,
    ) -> Result<BTreeMap<String, String>, String> {
        let profile = self.env_profile(active_env)?;
        let mut out = BTreeMap::new();
        let mut merge = |namespace: &str, table: &BTreeMap<String, String>| {
            for (key, value) in table {
                out.insert(format!("{namespace}:{key}"), value.clone());
            }
        };
        merge("url", &self.url);
        merge("vars", &self.vars);
        if let Some(profile) = profile {
            merge("url", &profile.url);
            merge("vars", &profile.vars);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    // Why: unwrap/expect are acceptable in `#[cfg(test)]` — a broken assumption
    // surfaces as a test failure, which is exactly the intent.
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn parse(text: &str) -> ProjectConfig {
        toml::from_str(text).expect("valid config")
    }

    #[test]
    fn base_vars_and_url_resolve_without_an_env() {
        let config =
            parse("[url]\nbase = \"http://localhost:3000\"\n[vars]\napiVersion = \"v1\"\n");
        let scope = config.config_vars(None).unwrap();
        assert_eq!(scope["url:base"], "http://localhost:3000");
        assert_eq!(scope["vars:apiVersion"], "v1");
    }

    #[test]
    fn env_overrides_deep_merge_over_the_base() {
        let config = parse(
            "[url]\nbase = \"http://localhost:3000\"\n\
             [vars]\napiVersion = \"v1\"\nusername = \"dev@x\"\n\
             [env.prod.url]\nbase = \"https://api.x\"\n\
             [env.prod.vars]\nusername = \"rel@x\"\n",
        );
        let scope = config.config_vars(Some("prod")).unwrap();
        assert_eq!(scope["url:base"], "https://api.x", "url overridden");
        assert_eq!(scope["vars:username"], "rel@x", "var overridden");
        assert_eq!(
            scope["vars:apiVersion"], "v1",
            "unlisted var inherited from base"
        );
    }

    #[test]
    fn env_http_override_is_field_wise() {
        let config = parse(
            "[http]\ntimeout-ms = 30000\nfollow-location = true\n\
             [env.prod.http]\ntimeout-ms = 60000\n",
        );
        let http = config.http_defaults(Some("prod")).unwrap();
        assert_eq!(http.timeout_ms, 60000, "env overrides timeout");
        assert!(http.follow_location, "follow-location inherited from base");
    }

    #[test]
    fn env_run_jobs_override_and_flag_wins() {
        let config = parse("[run]\njobs = 8\n[env.prod.run]\njobs = 2\n");
        assert_eq!(config.jobs(None, Some("prod")).unwrap(), 2, "env over base");
        assert!(
            config.jobs(None, Some("nonexistent")).is_err(),
            "unknown env errors"
        );
        assert_eq!(
            config.jobs(Some(16), Some("prod")).unwrap(),
            16,
            "flag wins"
        );
    }

    #[test]
    fn unknown_env_is_a_user_error_listing_known_names() {
        let config = parse("[env.staging.url]\nbase = \"x\"\n[env.prod.url]\nbase = \"y\"\n");
        let err = config.config_vars(Some("prd")).unwrap_err();
        assert!(err.contains("unknown environment `prd`"), "{err}");
        assert!(err.contains("prod") && err.contains("staging"), "{err}");
    }

    /// **The rule, pinned once for every key that obeys it.**
    ///
    /// Every path written in `proef.toml` resolves against the file's own
    /// directory. This used to be true of `[run] fragments` alone, so the same
    /// relative spelling meant two different directories depending on which key
    /// it sat under — from a subdirectory `fragments = "hurl"` resolved and
    /// `suite = "features"` reported "neither a feature file nor a directory".
    /// A key added later that forgets to call `resolve` fails here.
    #[test]
    fn every_config_written_path_resolves_against_the_config_file() {
        // Spelled for the host platform: a bare `/proj` is not absolute on
        // Windows, so a Unix-shaped fixture would assert nothing there.
        let project = if cfg!(windows) { r"C:\proj" } else { "/proj" };
        let mut config = parse(
            "[run]\nsuite = \"features\"\nfragments = \"hurl\"\nruns-dir = \"out\"\n\
             setup = \"phases/setup.feature\"\nteardown = \"phases/teardown.feature\"\n",
        );
        config.path = Some(Path::new(project).join("proef.toml"));

        let at = |rest: &str| Some(Path::new(project).join(rest));
        assert_eq!(config.default_suite_path(), at("features"), "[run] suite");
        assert_eq!(config.fragments(), at("hurl"), "[run] fragments");
        assert_eq!(
            config.runs_dir(),
            Path::new(project).join("out"),
            "runs-dir"
        );
        assert_eq!(config.setup(), at("phases/setup.feature"), "[run] setup");
        assert_eq!(
            config.teardown(),
            at("phases/teardown.feature"),
            "[run] teardown"
        );
        assert_eq!(config.state_file(), at(".proef-state.json").unwrap());
        assert_eq!(config.secrets_file(), at(".proef-secrets.json").unwrap());
    }

    /// No `proef.toml` in scope leaves every path relative — which is the
    /// working directory. The reference corpus is config-independent by design
    /// and several suites run it from a temp cwd with no config in scope, so a
    /// resolver that invented a root would break them.
    #[test]
    fn without_a_config_file_paths_stay_relative_to_the_working_directory() {
        let config = parse("[run]\nsuite = \"e2e\"\nruns-dir = \"out\"\n");
        assert_eq!(config.default_suite_path(), Some(PathBuf::from("e2e")));
        assert_eq!(config.runs_dir(), PathBuf::from("out"));
        assert_eq!(config.state_file(), PathBuf::from(".proef-state.json"));
        assert_eq!(
            ProjectConfig::default().runs_dir(),
            PathBuf::from(".proef-runs")
        );
    }

    /// An absolute path is taken as written — a record store or corpus may sit
    /// outside the project entirely.
    #[test]
    fn an_absolute_written_path_is_never_rerooted() {
        let (project, elsewhere) = if cfg!(windows) {
            (r"C:\proj", r"C:\elsewhere\runs")
        } else {
            ("/proj", "/elsewhere/runs")
        };
        let mut config = parse(&format!("[run]\nruns-dir = '{elsewhere}'\n"));
        config.path = Some(Path::new(project).join("proef.toml"));
        assert_eq!(config.runs_dir(), PathBuf::from(elsewhere));
    }

    #[test]
    fn default_suite_path_prefers_run_suite_then_tests_convention() {
        // Explicit `[run] suite` wins, no filesystem probe involved.
        let config = parse("[run]\nsuite = \"e2e\"\n");
        assert_eq!(config.default_suite_path(), Some(PathBuf::from("e2e")));

        // With no `[run] suite`, the fallback is the `tests/` convention —
        // exercised in a tempdir so the result can't depend on this crate's
        // own `tests/` directory (nextest runs unit tests with the crate
        // manifest dir as cwd, and `crates/proef-cli/tests/` exists).
        let tmp = tempfile::tempdir().expect("create tempdir");
        let prev = std::env::current_dir().expect("read cwd");
        std::env::set_current_dir(&tmp).expect("cd into tempdir");

        assert_eq!(
            ProjectConfig::default().default_suite_path(),
            None,
            "no tests/ dir present"
        );
        std::fs::create_dir("tests").expect("create tests/ dir");
        assert_eq!(
            ProjectConfig::default().default_suite_path(),
            Some(PathBuf::from("tests")),
            "tests/ convention found"
        );

        std::env::set_current_dir(prev).expect("restore cwd");
    }

    /// `fragments()` returns a **resolvable** path, always — never a display
    /// spelling.
    ///
    /// `proef lsp` hands this to `DiskSourceProvider`, which keys document
    /// identity on absolute source names: `documents::name_to_url` returns
    /// `None` for a relative path, so a shortened root silently costs
    /// go-to-definition on every `ref:` and drops `.hurl`-positioned
    /// diagnostics — the exact capability the fragment LSP work shipped. It
    /// regressed once by shortening here for the sake of the run record; how a
    /// path is *spelled* in artifacts belongs to `front::fragment_sources`.
    #[test]
    fn the_fragment_root_stays_resolvable_for_the_editor() {
        // Spelled for the host platform. A bare `/proj` is **not** absolute on
        // Windows — absolute there means a drive or UNC prefix — so a
        // Unix-shaped fixture would assert nothing on the one platform whose
        // path rules differ, which is the vacuity this test exists to prevent.
        // The corpus root is a TOML *literal* string (single quotes): `\c` is
        // not a valid escape in a basic string.
        let (project, corpus) = if cfg!(windows) {
            (r"C:\proj", r"C:\corpus\hurl")
        } else {
            ("/proj", "/corpus/hurl")
        };

        let mut config = parse("[run]\nfragments = \"tests/hurl\"\n");
        config.path = Some(Path::new(project).join("proef.toml"));
        let root = config.fragments().expect("configured");
        assert_eq!(root, Path::new(project).join("tests/hurl"));
        assert!(
            root.is_absolute(),
            "a relative root makes every fragment URI unformable: {root:?}"
        );

        // An absolute `[run] fragments` is taken as written — the corpus may
        // sit outside the project entirely.
        let mut config = parse(&format!("[run]\nfragments = '{corpus}'\n"));
        config.path = Some(Path::new(project).join("proef.toml"));
        assert_eq!(config.fragments(), Some(PathBuf::from(corpus)));
    }

    #[test]
    fn sla_thresholds_default_to_no_gate_and_env_overrides_field_wise() {
        // No `[sla]` table → inert (both ceilings unset).
        assert!(
            !ProjectConfig::default()
                .sla_thresholds(None)
                .unwrap()
                .is_set()
        );

        let config = parse(
            "[sla]\np95-ms = 250\nmax-ms = 1000\n\
             [env.staging.sla]\np95-ms = 800\n",
        );
        let base = config.sla_thresholds(None).unwrap();
        assert_eq!(base.p95_ms, Some(250));
        assert_eq!(base.max_ms, Some(1000));

        let staging = config.sla_thresholds(Some("staging")).unwrap();
        assert_eq!(staging.p95_ms, Some(800), "env overrides p95");
        assert_eq!(
            staging.max_ms,
            Some(1000),
            "unlisted max inherited from base"
        );
    }

    #[test]
    fn setup_and_teardown_are_absent_unless_configured() {
        assert_eq!(ProjectConfig::default().setup(), None);
        assert_eq!(ProjectConfig::default().teardown(), None);
    }

    #[test]
    fn unknown_top_level_key_is_rejected() {
        assert!(toml::from_str::<ProjectConfig>("[nonsense]\nx = 1\n").is_err());
    }

    #[test]
    fn env_run_rejects_non_jobs_overrides() {
        // `runs-dir` and `suite` are project-wide, not env-scoped: listing them
        // under `[env.<name>.run]` must fail loudly (deny_unknown_fields), never
        // parse-then-silently-drop the way reusing the full `RunTable` did.
        assert!(
            toml::from_str::<ProjectConfig>("[env.prod.run]\nsuite = \"e2e\"\n").is_err(),
            "per-env suite override must be rejected, not silently ignored"
        );
        assert!(
            toml::from_str::<ProjectConfig>("[env.prod.run]\nruns-dir = \"out\"\n").is_err(),
            "per-env runs-dir override must be rejected, not silently ignored"
        );
        assert!(
            toml::from_str::<ProjectConfig>("[env.prod.run]\nkeep-runs = 5\n").is_err(),
            "retention follows runs-dir: one record store, one policy"
        );
        // `jobs` remains the one env-overridable run field.
        let config = parse("[env.prod.run]\njobs = 3\n");
        assert_eq!(config.jobs(None, Some("prod")).unwrap(), 3);
    }

    /// `[run] keep-runs` makes the retention ceiling expressible. It was always
    /// a policy — the number just lived in a `const` where no project could
    /// reach it, and an adopting suite accumulated records for a day before
    /// noticing there was a ceiling at all.
    #[test]
    fn keep_runs_defaults_and_accepts_zero() {
        assert_eq!(
            parse("").keep_runs(),
            crate::exec::DEFAULT_KEEP_RUNS,
            "an unset key keeps the archive-sized default"
        );
        assert_eq!(parse("[run]\nkeep-runs = 5\n").keep_runs(), 5);
        // Zero is meaningful, not a mistake to reject: keep nothing but the run
        // in flight. That is the setting a `--watch` loop wants.
        assert_eq!(parse("[run]\nkeep-runs = 0\n").keep_runs(), 0);
    }
}
