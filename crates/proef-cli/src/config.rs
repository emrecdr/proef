//! Project configuration: `proef.toml` (TECH-SPEC §10 precedence — defaults <
//! `proef.toml` < environment < flags).

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
}

/// `[run]` settings.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunTable {
    /// Parallel scenario workers (default: available parallelism).
    pub jobs: Option<usize>,
    /// Run-record directory (default `.proef-runs`).
    #[serde(rename = "runs-dir")]
    pub runs_dir: Option<String>,
}

/// `[http]` settings (batch-level defaults; per-entry `[Options]` override).
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpTable {
    /// Per-request timeout in milliseconds.
    #[serde(rename = "timeout-ms")]
    pub timeout_ms: Option<u64>,
    /// Follow redirects.
    #[serde(rename = "follow-location")]
    pub follow_location: Option<bool>,
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

    /// The effective HTTP defaults.
    pub fn http_defaults(&self) -> HttpDefaults {
        let base = HttpDefaults::default();
        HttpDefaults {
            timeout_ms: self.http.timeout_ms.unwrap_or(base.timeout_ms),
            follow_location: self.http.follow_location.unwrap_or(base.follow_location),
        }
    }

    /// The effective job count (flag > config > available parallelism).
    pub fn jobs(&self, flag: Option<usize>) -> usize {
        flag.or(self.run.jobs)
            .unwrap_or_else(|| {
                std::thread::available_parallelism().map_or(4, std::num::NonZero::get)
            })
            .max(1)
    }

    /// The run-record directory.
    pub fn runs_dir(&self) -> &str {
        self.run.runs_dir.as_deref().unwrap_or(".proef-runs")
    }
}
