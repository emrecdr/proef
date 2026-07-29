//! `proef` — declarative, modular, multi-engine end-to-end test runner.
//!
//! The CLI is the orchestrating edge: it assembles the engine registry (one
//! line per engine, cargo-feature-gated — ADR-0002), owns process exit codes
//! (ADR-0009), performs all IO (core purity), and is the only crate rendering
//! user-facing diagnostics (miette — ADR-0009).

mod ci_reports;
mod commands;
mod config;
mod exec;
mod explain;
mod fmt;
mod front;
mod registry;
mod render;
mod secretstore;
mod watch;

use std::path::PathBuf;

use clap::{Parser, Subcommand};

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
        /// A .feature file or a directory tree containing them
        path: PathBuf,
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
        #[arg(long)]
        output: Option<String>,
        /// `JUnit` XML: a path, or `auto` (run dir, only under `GITHUB_ACTIONS`)
        #[arg(long)]
        junit: Option<String>,
        /// Only the scenario with exactly this name
        #[arg(long)]
        scenario: Option<String>,
        /// Rerun on feature/pack changes (Ctrl-C to stop)
        #[arg(long)]
        watch: bool,
    },
    /// List every scenario (flow) with its anchor and tags
    Flows {
        /// A .feature file or a directory tree containing them
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Machine output: `json` prints one object per scenario
        #[arg(long)]
        output: Option<String>,
    },
    /// Emit canonical .hurl artifacts + sidecars for a stable hand-off
    Artifacts {
        /// A .feature file or a directory tree containing them
        path: PathBuf,
        /// Output directory for .hurl / .map.json / .vars files
        #[arg(short, long)]
        output: PathBuf,
        /// Override the injected run id (deterministic artifacts for CI)
        #[arg(long)]
        run_id: Option<String>,
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
}

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
            watch: watch_mode,
        } => {
            let run_once = |cancel| {
                if dry_run {
                    commands::dry_run(&path, &tags, scenario.as_deref())
                } else {
                    exec::execute(
                        &path,
                        &tags,
                        jobs,
                        output.as_deref() == Some("json"),
                        junit.as_deref(),
                        scenario.as_deref(),
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
        Command::Flows { path, output } => {
            commands::flows(&path, output.as_deref() == Some("json"))
        }
        Command::Artifacts {
            path,
            output,
            run_id,
        } => commands::artifacts(&path, &output, run_id),
        Command::Schema { add_to } => commands::schema(&add_to),
        Command::Doctor => commands::doctor(&registry::engines()),
        Command::Secret { action } => {
            let result = match action {
                SecretAction::Set { name, value } => secretstore::set(&name, value.as_deref()),
                SecretAction::List => secretstore::list(),
            };
            match result {
                Ok(()) => proef_core::error::ExitCode::Success,
                Err(message) => {
                    eprintln!("error: {message}");
                    proef_core::error::ExitCode::UserError
                }
            }
        }
        Command::Explain { run_id } => {
            let config = config::ProjectConfig::load().unwrap_or_default();
            explain::explain(config.runs_dir(), run_id.as_deref())
        }
        Command::Fmt { path, check } => fmt::fmt(&path, check),
    };
    std::process::ExitCode::from(code.code())
}
