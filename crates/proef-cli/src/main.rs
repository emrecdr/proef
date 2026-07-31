//! `proef` — declarative, modular, multi-engine end-to-end test runner.
//!
//! The CLI is the orchestrating edge: it assembles the engine registry (one
//! line per engine, cargo-feature-gated — ADR-0002), owns process exit codes
//! (ADR-0009), performs all IO (core purity), and is the only crate rendering
//! user-facing diagnostics (miette — ADR-0009).

mod assets;
mod ci_reports;
mod commands;
mod config;
mod exec;
mod explain;
mod fmt;
mod front;
mod fsutil;
mod record;
mod registry;
mod render;
mod sarif;
mod secretstore;
mod watch;

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

/// Machine output formats (`--output`). A typed enum so an unknown value is a
/// clap usage error — exit 2 (ADR-0009) — never a silent fall-back to the
/// human report.
#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum OutputFormat {
    /// One JSON summary object (`test`) / one object per scenario (`flows`)
    Json,
}

#[derive(Parser)]
#[command(
    name = "proef",
    version,
    about = "Declarative, modular, multi-engine end-to-end test runner",
    arg_required_else_help = true
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Validate and run feature files against the configured target
    Test {
        /// A .feature file or directory (default: `[run] suite`, else `tests/`)
        path: Option<PathBuf>,
        /// Validate everything (bind + lower + emit + parse), execute nothing
        #[arg(long)]
        dry_run: bool,
        /// Only scenarios with any of these tags (csv, OR semantics)
        #[arg(long, value_delimiter = ',')]
        tags: Vec<String>,
        /// Parallel scenario workers (default: proef.toml or CPU count)
        #[arg(long)]
        jobs: Option<usize>,
        /// Machine output: `json` prints a summary object to stdout
        #[arg(long, value_enum)]
        output: Option<OutputFormat>,
        /// `JUnit` XML: a path, or `auto` (run dir, only under `GITHUB_ACTIONS`)
        #[arg(long)]
        junit: Option<String>,
        /// Only the scenario with exactly this name
        #[arg(long)]
        scenario: Option<String>,
        /// Only scenarios from exactly this feature file (as printed by
        /// `proef flows`; combine with --scenario to pin one of several
        /// same-named scenarios across files)
        #[arg(long)]
        scenario_file: Option<String>,
        /// Rerun on feature/pack changes (Ctrl-C to stop)
        #[arg(long)]
        watch: bool,
        /// Pin the injected run id: reproducible fake data and a stable run record
        #[arg(long)]
        run_id: Option<String>,
        /// Write validation diagnostics as a SARIF 2.1.0 log (requires --dry-run)
        #[arg(long, requires = "dry_run")]
        sarif: Option<PathBuf>,
        /// Re-run only the scenarios that failed in the last run
        #[arg(long)]
        rerun: bool,
        /// Select a `[env.<name>]` profile from `proef.toml` (or set `PROEF_ENV`)
        #[arg(long)]
        env: Option<String>,
    },
    /// List every scenario (flow) with its anchor and tags
    Flows {
        /// A .feature file or directory (default: `[run] suite`, else `tests/`)
        path: Option<PathBuf>,
        /// Machine output: `json` prints one object per scenario
        #[arg(long, value_enum)]
        output: Option<OutputFormat>,
        /// Select a `[env.<name>]` profile from `proef.toml` (or set `PROEF_ENV`)
        #[arg(long)]
        env: Option<String>,
    },
    /// List every macro with its call count, flagging pattern macros nothing binds
    Macros {
        /// A .feature file or directory (default: `[run] suite`, else `tests/`)
        path: Option<PathBuf>,
        /// Machine output: `json` prints one object per macro
        #[arg(long, value_enum)]
        output: Option<OutputFormat>,
        /// Select a `[env.<name>]` profile from `proef.toml` (or set `PROEF_ENV`)
        #[arg(long)]
        env: Option<String>,
    },
    /// Emit canonical .hurl artifacts + sidecars for a stable hand-off
    Artifacts {
        /// A .feature file or directory (default: `[run] suite`, else `tests/`)
        path: Option<PathBuf>,
        /// Output directory for .hurl / .map.json / .vars files
        #[arg(short, long)]
        output: PathBuf,
        /// Override the injected run id (deterministic artifacts for CI)
        #[arg(long)]
        run_id: Option<String>,
        /// Select a `[env.<name>]` profile from `proef.toml` (or set `PROEF_ENV`)
        #[arg(long)]
        env: Option<String>,
    },
    /// Print the pack JSON Schema (or install it next to pack files)
    Schema {
        /// Write the schema next to these pack files and add editor modelines
        #[arg(long = "add-to", num_args = 1..)]
        add_to: Vec<PathBuf>,
    },
    /// Check native libraries and environment prerequisites for all registered engines
    Doctor,
    /// Manage the encrypted secret store (values never appear in artifacts)
    Secret {
        #[command(subcommand)]
        action: SecretAction,
    },
    /// Summarize a run from its event record
    Explain {
        /// Run id (default: the latest run)
        run_id: Option<String>,
    },
    /// Normalize the raw hurl blocks inside macro packs
    Fmt {
        /// A pack file, a packs/ dir, or a suite dir containing packs/
        path: PathBuf,
        /// Report files needing formatting without rewriting (exit 1 if any)
        #[arg(long)]
        check: bool,
    },
}

#[derive(Subcommand)]
enum SecretAction {
    /// Store an encrypted value (prompted hidden unless --value is given)
    Set {
        /// Secret name (referenced as ${secret:NAME} in packs)
        name: String,
        /// Value (plumbing for scripts; prefer the hidden prompt)
        #[arg(long)]
        value: Option<String>,
    },
    /// List stored secret names (never values)
    List,
    /// Remove a stored secret
    Rm {
        /// Secret name to remove
        name: String,
    },
}

/// Resolve the suite path when a command is given none: `[run] suite` from
/// proef.toml, else the `tests/` convention. An explicit path always wins; a
/// missing default with no `tests/` present is a user error (exit 2) — never a
/// silent no-op.
fn resolve_suite_path(
    path: Option<PathBuf>,
    config: &config::ProjectConfig,
) -> Result<PathBuf, proef_core::error::ExitCode> {
    if let Some(path) = path {
        return Ok(path);
    }
    if let Some(suite) = config.suite() {
        return Ok(PathBuf::from(suite));
    }
    let convention = PathBuf::from("tests");
    if convention.is_dir() {
        return Ok(convention);
    }
    eprintln!(
        "error: no path given and no default suite found — pass a path, set `[run] suite` in proef.toml, or create a `tests/` directory"
    );
    Err(proef_core::error::ExitCode::UserError)
}

/// Load `proef.toml` once per invocation (absent file = defaults; a malformed
/// file is a user error). Threaded into suite resolution and the command so the
/// config is read a single time, not once per consumer.
fn load_config() -> Result<config::ProjectConfig, proef_core::error::ExitCode> {
    config::ProjectConfig::load().map_err(|message| {
        eprintln!("error: {message}");
        proef_core::error::ExitCode::UserError
    })
}

/// The active environment: the `--env` flag wins, else `PROEF_ENV`, else none.
fn active_env(flag: Option<String>) -> Option<String> {
    flag.or_else(|| std::env::var("PROEF_ENV").ok())
}

/// The shared preamble of every suite command (`test`/`flows`/`artifacts`):
/// load config once, resolve the suite path, and pick the active environment.
fn prepare(
    path: Option<PathBuf>,
    env: Option<String>,
) -> Result<(config::ProjectConfig, PathBuf, Option<String>), proef_core::error::ExitCode> {
    let config = load_config()?;
    let path = resolve_suite_path(path, &config)?;
    Ok((config, path, active_env(env)))
}

// One dispatch table over the CLI surface; splitting arms hides the routing.
#[allow(clippy::too_many_lines)]
fn main() -> std::process::ExitCode {
    render::install();
    // clap renders usage errors itself and exits 2 — which is exactly the
    // user-error contract (ADR-0009); the mapping is pinned by tests/cli.rs.
    let cli = Cli::parse();
    let code = match cli.command {
        Command::Test {
            path,
            dry_run,
            tags,
            jobs,
            output,
            junit,
            scenario,
            scenario_file,
            watch: watch_mode,
            run_id,
            sarif,
            rerun,
            env,
        } => match prepare(path, env) {
            Err(code) => code,
            Ok((config, path, active_env)) => {
                let run_once = |cancel| {
                    if dry_run {
                        commands::dry_run(
                            &path,
                            &tags,
                            scenario.as_deref(),
                            scenario_file.as_deref(),
                            active_env.as_deref(),
                            run_id.clone(),
                            sarif.as_deref(),
                            &config,
                        )
                    } else {
                        exec::execute(
                            &path,
                            &tags,
                            jobs,
                            output == Some(OutputFormat::Json),
                            junit.as_deref(),
                            scenario.as_deref(),
                            scenario_file.as_deref(),
                            active_env.as_deref(),
                            run_id.clone(),
                            rerun,
                            &config,
                            cancel, // None = execute installs its own Ctrl-C handler
                        )
                    }
                };
                if watch_mode {
                    // The loop owns Ctrl-C and hands each run its token.
                    watch::watch_loop(&path, |token| run_once(Some(token)))
                } else {
                    run_once(None)
                }
            }
        },
        Command::Flows { path, output, env } => match prepare(path, env) {
            Err(code) => code,
            Ok((config, path, active_env)) => commands::flows(
                &path,
                output == Some(OutputFormat::Json),
                active_env.as_deref(),
                &config,
            ),
        },
        Command::Macros { path, output, env } => match prepare(path, env) {
            Err(code) => code,
            Ok((config, path, active_env)) => commands::macros(
                &path,
                output == Some(OutputFormat::Json),
                active_env.as_deref(),
                &config,
            ),
        },
        Command::Artifacts {
            path,
            output,
            run_id,
            env,
        } => match prepare(path, env) {
            Err(code) => code,
            Ok((config, path, active_env)) => {
                commands::artifacts(&path, &output, run_id, active_env.as_deref(), &config)
            }
        },
        Command::Schema { add_to } => commands::schema(&add_to),
        Command::Doctor => commands::doctor(&registry::engines()),
        Command::Secret { action } => {
            let result = match action {
                SecretAction::Set { name, value } => secretstore::set(&name, value.as_deref()),
                SecretAction::List => secretstore::list(),
                SecretAction::Rm { name } => secretstore::rm(&name),
            };
            match result {
                Ok(()) => proef_core::error::ExitCode::Success,
                Err(err) => {
                    eprintln!("error: {}", err.message());
                    // The variant carries the ADR-0009 classification: a typo
                    // exits 2, an unwritable key dir or lock failure exits 3.
                    match err {
                        secretstore::SecretError::User(_) => proef_core::error::ExitCode::UserError,
                        secretstore::SecretError::System(_) => {
                            proef_core::error::ExitCode::SystemError
                        }
                    }
                }
            }
        }
        Command::Explain { run_id } => {
            // Same loud failure as `test` (exec.rs): a malformed proef.toml
            // silently defaulting `runs-dir` would misdiagnose "no runs".
            match config::ProjectConfig::load() {
                Ok(config) => explain::explain(config.runs_dir(), run_id.as_deref()),
                Err(message) => {
                    eprintln!("error: {message}");
                    proef_core::error::ExitCode::UserError
                }
            }
        }
        Command::Fmt { path, check } => fmt::fmt(&path, check),
    };
    std::process::ExitCode::from(code.code())
}
