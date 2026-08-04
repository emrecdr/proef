//! The `proef lsp` subcommand: assemble the injected analysis inputs at the IO
//! edge and hand them to the sans-IO-fed language server over stdio.
//!
//! Everything the headless analysis needs — the engine-derived step kinds, the
//! `${env:…}`/`${url:…}`/`${vars:…}` scopes, and the disk-backed source
//! provider — is built here, at the process boundary, and injected. The core
//! stays sans-IO; the LSP is a second front-end over it, mirroring how a normal
//! run assembles the same inputs (`front::run`). The analysis root is the
//! configured suite (`ProjectConfig::default_suite_path`), not the whole
//! working-directory tree — the same convention `resolve_suite_path` uses for
//! `proef test`, so the two never diverge.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use proef_core::error::ExitCode;

use crate::config::ProjectConfig;
use crate::disk_provider::DiskSourceProvider;
use crate::registry;

pub fn run() -> ExitCode {
    // Engine-derived step kinds + kind→engine routing, assembled exactly as a
    // normal run does (front::run) so packs validate and lower identically —
    // one implementation of this mapping, not a divergent copy. Empty kinds
    // would make every `hurl:` step an unknown kind and starve the LSP of any
    // useful analysis, so this is the load-bearing wiring.
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

    // Load proef.toml once — it drives both the suite root and the ${url}/${vars} scope.
    let config = ProjectConfig::load().unwrap_or_default();
    let active_env = std::env::var("PROEF_ENV").ok();

    // Root at the configured suite ([run] suite, else the tests/ convention),
    // made absolute against cwd and left uncanonicalized so source names stay
    // identical to the client's document URIs (canonicalizing would resolve
    // symlinks and desync them). Falls back to cwd when no suite resolves, so
    // the server always starts. Scoping here keeps the analyzer off paths
    // outside the resolved suite (target/, docs/, …) — it does NOT exclude
    // tests/errors/, the deliberately-broken fixture corpus, which lives
    // inside the tests/ convention. What keeps that corpus from blanking the
    // analysis is degradation (analyze_suite keeps whatever packs load
    // instead of discarding the whole set on one broken pack), not scoping.
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let root = config
        .default_suite_path()
        .map(|rel| {
            if rel.is_absolute() {
                rel
            } else {
                cwd.join(rel)
            }
        })
        .unwrap_or(cwd);

    // Injected environment snapshot (`${env:…}`), assembled like front::run: a
    // foreign non-UTF-8 variable must not abort startup — it can never match a
    // UTF-8 reference anyway.
    let env: BTreeMap<String, String> = std::env::vars_os()
        .filter_map(|(key, value)| Some((key.into_string().ok()?, value.into_string().ok()?)))
        .collect();

    // v1: config vars are a startup snapshot — editing `proef.toml` needs a
    // server restart to re-read.
    let config_vars = config
        .config_vars(active_env.as_deref())
        .unwrap_or_default();

    let cfg = proef_lsp::ServerConfig {
        transport: proef_lsp::Transport::Stdio,
        root: root.clone(),
        disk: Box::new(DiskSourceProvider::new(root)),
        kinds,
        kind_to_engine,
        env,
        config_vars,
        debounce: Duration::from_millis(200),
    };

    match proef_lsp::run(cfg) {
        Ok(()) => ExitCode::Success,
        Err(err) => {
            eprintln!("proef lsp: {err}");
            ExitCode::SystemError
        }
    }
}
