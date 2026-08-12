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

/// The config this server analyses against.
///
/// `--config` wins over discovery, including over the workspace root the client
/// announces: the flag names a file, and a named file is not a guess to be
/// improved on. Without that precedence the editor and the runner disagree in
/// exactly the layout `--config` exists for — a config beside the suite, which
/// discovery cannot reach from the repository root — so `proef test --config …`
/// runs green while every `ref:` reads as unknown in the editor.
///
/// Unreadable or absent falls back to defaults, as before: `proef lsp` must
/// start anywhere, and a server that refuses to boot is worse than one
/// analysing a project with no config.
fn load_config(
    explicit: Option<&std::path::Path>,
    client_root: Option<&std::path::Path>,
) -> ProjectConfig {
    if let Some(path) = explicit {
        return ProjectConfig::load_at(path).unwrap_or_default();
    }
    match client_root {
        Some(root) => ProjectConfig::load_from(root).unwrap_or_default(),
        None => ProjectConfig::load().unwrap_or_default(),
    }
}

pub fn run(explicit_config: Option<std::path::PathBuf>) -> ExitCode {
    // Engine-derived step kinds + kind→engine routing, assembled exactly as a
    // normal run does (front::run) so packs validate and lower identically —
    // one implementation of this mapping, not a divergent copy. Empty kinds
    // would make every `hurl:` step an unknown kind and starve the LSP of any
    // useful analysis, so this is the load-bearing wiring.
    let kinds = registry::step_kinds();
    let kind_to_engine = registry::kind_to_engine();

    // Load proef.toml once — it drives both the suite root and the ${url}/${vars} scope.
    let config = load_config(explicit_config.as_deref(), None);
    // A malformed PROEF_ENV must stop the server from starting rather than
    // silently analyse against the wrong config profile — an editor showing
    // diagnostics from the wrong environment is the "reports the wrong
    // cause" failure this module exists to avoid.
    let active_env = match crate::envvar::read("PROEF_ENV") {
        Ok(value) => value,
        Err(message) => {
            crate::render::errln!("error: {message}");
            return ExitCode::UserError;
        }
    };

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
        disk: Box::new(DiskSourceProvider::new(root).with_fragments(config.fragments())),
        kinds,
        kind_to_engine,
        env,
        config_vars,
        debounce: Duration::from_millis(200),
        // Re-resolve once the client says where its workspace is. The root
        // above is a guess from the process working directory, which is wrong
        // whenever the editor was launched somewhere other than the project —
        // `nvim ~/proj/x.feature` from `$HOME` rooted the analyser at `$HOME`.
        // Same resolution order as above, just anchored on the client's root:
        // `[run] suite`, else the `tests/` convention, else the root itself.
        // Owned: the callback outlives this frame and must carry the flag
        // with it, or the client's announced root would silently win over a
        // file the user named.
        resolve_root: Some(Box::new(move |client_root: &std::path::Path| {
            let config = load_config(explicit_config.as_deref(), Some(client_root));
            let root = config.default_suite_path().map_or_else(
                || client_root.to_path_buf(),
                |rel| {
                    if rel.is_absolute() {
                        rel
                    } else {
                        client_root.join(rel)
                    }
                },
            );
            let disk: Box<dyn proef_core::provider::SourceProvider + Send> =
                Box::new(DiskSourceProvider::new(root.clone()).with_fragments(config.fragments()));
            (root, disk)
        })),
    };

    match proef_lsp::run(cfg) {
        Ok(()) => ExitCode::Success,
        Err(err) => {
            crate::render::errln!("proef lsp: {err}");
            ExitCode::SystemError
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::load_config;

    /// `--config` outranks discovery, including the workspace root the client
    /// announces.
    ///
    /// The precedence is the whole point: discovery only searches *up*, so a
    /// config beside the suite is unreachable from the repository root — which
    /// is the layout the flag exists for. Without this, `proef test --config …`
    /// runs green while the editor loads a different config (or none) and
    /// reports every `ref:` as unknown, and diagnostics that disagree with the
    /// runner are worse than no diagnostics.
    #[test]
    fn an_explicit_config_outranks_both_discovery_and_the_client_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("nested")).unwrap();
        let named = root.join("nested/proef.toml");
        std::fs::write(&named, "[run]\nsuite = \"from-explicit\"\n").unwrap();
        // A second config the client root *would* find, to prove which won.
        std::fs::write(
            root.join("proef.toml"),
            "[run]\nsuite = \"from-discovery\"\n",
        )
        .unwrap();

        let explicit = load_config(Some(&named), Some(root));
        assert_eq!(explicit.path(), Some(named.as_path()));
        assert_eq!(explicit.run.suite.as_deref(), Some("from-explicit"));

        // Without the flag the client's root still decides, as before.
        let discovered = load_config(None, Some(root));
        assert_eq!(discovered.run.suite.as_deref(), Some("from-discovery"));
    }

    /// A named file that is not there falls back to defaults rather than
    /// refusing to boot: `proef lsp` must start anywhere, and a server that
    /// exits is worse than one analysing a project with no config. The runner
    /// makes the opposite call for the same flag — there a typo'd path is a
    /// user error — because a run that silently has no configuration produces
    /// wrong results, while an editor merely offers less.
    #[test]
    fn a_missing_named_config_leaves_the_server_startable() {
        let dir = tempfile::tempdir().unwrap();
        let config = load_config(Some(&dir.path().join("nope.toml")), None);
        assert_eq!(config.path(), None);
    }
}
