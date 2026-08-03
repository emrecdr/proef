//! The `proef lsp` subcommand: assemble the injected analysis inputs at the IO
//! edge and hand them to the sans-IO-fed language server over stdio.
//!
//! Everything the headless analysis needs — the engine-derived step kinds, the
//! `${env:…}`/`${url:…}`/`${vars:…}` scopes, and the disk-backed source
//! provider — is built here, at the process boundary, and injected. The core
//! stays sans-IO; the LSP is a second front-end over it, mirroring how a normal
//! run assembles the same inputs (`front::run`).

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

    // The suite root is the current directory — already absolute (the
    // absolute-path invariant the LSP keys every source name on), and left
    // uncanonicalized so source names stay identical to the client's document
    // URIs (canonicalizing would resolve symlinks and desync them). Computed
    // once and reused for both the provider and the config root.
    let root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    // Injected environment snapshot (`${env:…}`), assembled like front::run: a
    // foreign non-UTF-8 variable must not abort startup — it can never match a
    // UTF-8 reference anyway.
    let env: BTreeMap<String, String> = std::env::vars_os()
        .filter_map(|(key, value)| Some((key.into_string().ok()?, value.into_string().ok()?)))
        .collect();

    // v1: config vars are a startup snapshot — editing `proef.toml` needs a
    // server restart to re-read. Loaded through the CLI's own config path.
    let config_vars = load_config_vars_snapshot();

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

/// Build the `${url:…}` / `${vars:…}` scope the same way the suite commands do
/// (`ProjectConfig::config_vars` for the active environment). Best-effort at the
/// LSP edge: a missing or malformed `proef.toml`, or an unknown `PROEF_ENV`,
/// falls back to an empty scope so the server still analyzes — config-backed
/// `${url:}`/`${vars:}` references may then warn until the file is fixed and the
/// server restarted.
fn load_config_vars_snapshot() -> BTreeMap<String, String> {
    // The active environment mirrors the CLI: no `--env` flag exists for `lsp`,
    // so `PROEF_ENV` is the only selector.
    let active_env = std::env::var("PROEF_ENV").ok();
    ProjectConfig::load()
        .and_then(|config| config.config_vars(active_env.as_deref()))
        .unwrap_or_default()
}
