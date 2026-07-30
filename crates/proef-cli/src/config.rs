//! Project configuration: `proef.toml` (TECH-SPEC §10 precedence — defaults <
//! `proef.toml` < active environment (`[env.<name>]`) < environment variables <
//! flags).
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
use std::path::Path;

use proef_core::engine::HttpDefaults;
use serde::Deserialize;

/// Loaded `proef.toml` (all fields optional — defaults apply).
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectConfig {
    /// `[run]` table.
    #[serde(default)]
    pub run: RunTable,
    /// `[http]` table.
    #[serde(default)]
    pub http: HttpTable,
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
    /// Default suite path used when `proef test` is given no path
    /// (falls back to the `tests/` convention when unset — see `suite`).
    pub suite: Option<String>,
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
    /// `[env.<name>.http]` — HTTP setting overrides.
    #[serde(default)]
    pub http: HttpTable,
    /// `[env.<name>.run]` — run setting overrides (`jobs` only; see [`RunOverride`]).
    #[serde(default)]
    pub run: RunOverride,
}

impl ProjectConfig {
    /// Load `proef.toml` from the working directory (absent file = defaults;
    /// a malformed file is a user error worth failing loudly on).
    pub fn load() -> Result<Self, String> {
        let path = Path::new("proef.toml");
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(path)
            .map_err(|err| format!("cannot read proef.toml: {err}"))?;
        toml::from_str(&text).map_err(|err| format!("proef.toml is invalid: {err}"))
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

    /// The run-record directory (`[run] runs-dir`, default `.proef-runs`).
    /// Not environment-scoped — one project shares one record store.
    pub fn runs_dir(&self) -> &str {
        self.run.runs_dir.as_deref().unwrap_or(".proef-runs")
    }

    /// The configured default suite path (`[run] suite`), if any. The CLI falls
    /// back to the `tests/` convention when this is unset and no path is passed.
    pub fn suite(&self) -> Option<&str> {
        self.run.suite.as_deref()
    }

    /// The injected `${url:…}` / `${vars:…}` scope for the active environment,
    /// keyed `"<namespace>:<key>"` (deep-merged: base then env override). Passed
    /// into `LowerCtx::config_vars` so the sans-IO core resolves these without
    /// reading any file itself.
    pub fn config_vars(
        &self,
        active_env: Option<&str>,
    ) -> Result<BTreeMap<String, String>, String> {
        let profile = self.env_profile(active_env)?;
        let mut out = BTreeMap::new();
        for (key, value) in &self.url {
            out.insert(format!("url:{key}"), value.clone());
        }
        for (key, value) in &self.vars {
            out.insert(format!("vars:{key}"), value.clone());
        }
        if let Some(profile) = profile {
            for (key, value) in &profile.url {
                out.insert(format!("url:{key}"), value.clone());
            }
            for (key, value) in &profile.vars {
                out.insert(format!("vars:{key}"), value.clone());
            }
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

    #[test]
    fn suite_default_path_is_read() {
        let config = parse("[run]\nsuite = \"e2e\"\n");
        assert_eq!(config.suite(), Some("e2e"));
        assert_eq!(ProjectConfig::default().suite(), None);
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
        // `jobs` remains the one env-overridable run field.
        let config = parse("[env.prod.run]\njobs = 3\n");
        assert_eq!(config.jobs(None, Some("prod")).unwrap(), 3);
    }
}
