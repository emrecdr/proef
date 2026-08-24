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
mod diff;
mod disk_provider;
mod envvar;
mod exec;
mod explain;
mod flaky;
mod fmt;
mod front;
mod fsutil;
mod init;
mod lsp;
mod record;
mod registry;
mod render;
mod report;
mod sarif;
mod secretstore;
mod sla;
mod tap;
mod watch;

use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand, ValueEnum};

/// Hard-exit code on a second interrupt: 128 + SIGINT(2), the shell
/// convention. Deliberately outside the typed `ExitCode` taxonomy (ADR-0009
/// amendment) — not a graceful outcome, so it bypasses the enum entirely.
pub(crate) const INTERRUPT_EXIT_CODE: i32 = 130;

/// Machine output formats (`--output`). A typed enum so an unknown value is a
/// clap usage error — exit 2 (ADR-0009) — never a silent fall-back to the
/// human report.
#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum OutputFormat {
    /// One JSON summary object (`test`) / one object per scenario (`flows`)
    Json,
    /// TAP version 13 to stdout, one test point per scenario (`test` only) —
    /// pipe into `prove`/`tappy`. The human report moves to stderr.
    Tap,
}

/// Coerce `--output` for a command that only understands `json` (everything
/// except `test`): `tap` is `test`-only, so it is a user error here rather than
/// a silent fall-back to the human report.
fn json_only(output: Option<OutputFormat>) -> Result<bool, proef_core::error::ExitCode> {
    match output {
        None => Ok(false),
        Some(OutputFormat::Json) => Ok(true),
        Some(OutputFormat::Tap) => {
            crate::render::errln!("error: --output tap is only supported by `proef test`");
            Err(proef_core::error::ExitCode::UserError)
        }
    }
}

#[derive(Parser)]
#[command(
    name = "proef",
    version,
    about = "Declarative, modular, multi-engine end-to-end test runner",
    arg_required_else_help = true
)]
struct Cli {
    /// Read this proef.toml instead of searching up from the working directory
    #[arg(long, global = true, value_name = "PATH")]
    config: Option<PathBuf>,
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
        /// Select scenarios by a boolean tag expression, e.g.
        /// `"@api and not @slow"` (operators `and`/`or`/`not`, parentheses; the
        /// `@` is optional). Atoms may glob: `*` spans any run, `?` is one
        /// character, anchored — `@FRD-*` selects the whole family. Omitted,
        /// every scenario runs.
        #[arg(long)]
        tags: Option<String>,
        /// Parallel scenario workers (default: proef.toml or CPU count)
        #[arg(long)]
        jobs: Option<usize>,
        /// Machine output to stdout: `json` (a summary object) or `tap` (a TAP
        /// v13 stream, one point per scenario); the human report moves to stderr
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
        /// Re-run the last run's failures — and, when it was cancelled
        /// (`--max-fail`, Ctrl-C), the scenarios it never reached
        #[arg(long)]
        rerun: bool,
        /// Select a `[env.<name>]` profile from `proef.toml` (or set `PROEF_ENV`)
        #[arg(long)]
        env: Option<String>,
        /// Stop after N scenario failures: in-flight scenarios finish, the
        /// rest record as skipped, teardown still runs (`1` = fail fast)
        #[arg(long, value_parser = clap::value_parser!(u32).range(1..))]
        max_fail: Option<u32>,
        /// Run only this shard of the suite, e.g. `1/3` — scenarios are
        /// assigned by a stable hash of `(file, scenario)`, so adding one
        /// never re-buckets the others across a CI matrix
        #[arg(long, value_parser = parse_shard)]
        shard: Option<(u32, u32)>,
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
    /// List every macro with the sentence it binds and its call count, flagging pattern macros nothing binds
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
    /// List the .hurl fragment corpus with how many scenarios run each entry, flagging ones nothing reaches
    Fragments {
        /// A .feature file or directory (default: `[run] suite`, else `tests/`)
        path: Option<PathBuf>,
        /// Machine output: `json` prints one object per entry
        #[arg(long, value_enum)]
        output: Option<OutputFormat>,
        /// Exit 1 when a fragment exists that no scenario runs
        #[arg(long)]
        check: bool,
        /// With `--check`, also fail on entries carrying no `# @proef` annotation
        #[arg(long, requires = "check")]
        require_annotated: bool,
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
    /// Scaffold a minimal working suite in a new or existing directory
    Init {
        /// Target directory (default: the current directory)
        dir: Option<PathBuf>,
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
    /// Flakiness verdicts over the retained run history: flapping,
    /// passes-only-on-retry, always-failing
    Flaky {
        /// Machine output: `json` prints one object per scenario
        #[arg(long, value_enum)]
        output: Option<OutputFormat>,
    },
    /// Compare two run records: regressions, fixes, flakiness, perf deltas
    Diff {
        /// Base: a run id, a record directory, or an events .jsonl file
        /// (default: the previous run)
        base: Option<String>,
        /// New: a run id, a record directory, or an events .jsonl file
        /// (default: the latest run)
        new: Option<String>,
        /// Exit 1 when a scenario regressed (passed → failed), for CI gating
        #[arg(long)]
        fail_on_regression: bool,
    },
    /// Write a self-contained HTML report for a run from its event record
    Report {
        /// Run id (default: the latest run)
        run_id: Option<String>,
        /// Output file (default: `report.html` inside the run dir)
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Normalize the raw hurl blocks inside macro packs
    Fmt {
        /// A pack file, a packs/ dir, or a suite dir containing packs/
        path: PathBuf,
        /// Report files needing formatting without rewriting (exit 1 if any)
        #[arg(long)]
        check: bool,
    },
    /// Run the proef language server over stdio (diagnostics, definitions,
    /// completion, references for feature files and macro packs).
    Lsp,
}

#[derive(Subcommand)]
enum SecretAction {
    /// Store an encrypted value (hidden prompt, or --stdin for scripts)
    Set {
        /// Secret name (referenced as ${secret:NAME} in packs)
        name: String,
        /// Read the value from stdin instead of prompting, for scripts and CI.
        /// A command-line value would be visible to anyone who can run `ps`,
        /// so there is deliberately no flag that takes one.
        #[arg(long)]
        stdin: bool,
    },
    /// List stored secret names (never values)
    List,
    /// Remove a stored secret
    Rm {
        /// Secret name to remove
        name: String,
    },
}

/// Parse `--shard I/N` (1-based): `1/3` is the first of three shards.
/// Playwright's spelling, and the same validation everyone applies: both
/// halves at least 1, index within count.
fn parse_shard(raw: &str) -> Result<(u32, u32), String> {
    let (index, count) = raw
        .split_once('/')
        .ok_or_else(|| "expected I/N, e.g. --shard 1/3".to_owned())?;
    let index: u32 = index
        .parse()
        .map_err(|_| format!("`{index}` is not a shard index"))?;
    let count: u32 = count
        .parse()
        .map_err(|_| format!("`{count}` is not a shard count"))?;
    if index == 0 || count == 0 {
        return Err("shards are 1-based: the first of three is 1/3".to_owned());
    }
    if index > count {
        return Err(format!("shard {index}/{count}: index exceeds count"));
    }
    Ok((index, count))
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
    if let Some(suite) = config.default_suite_path() {
        return Ok(suite);
    }
    crate::render::errln!(
        "error: no path given and no default suite found — pass a path, set `[run] suite` in proef.toml, or create a `tests/` directory"
    );
    Err(proef_core::error::ExitCode::UserError)
}

/// Load `proef.toml` once per invocation (absent file = defaults; a malformed
/// file is a user error). Threaded into suite resolution and the command so the
/// config is read a single time, not once per consumer.
fn load_config(
    explicit: Option<&std::path::Path>,
) -> Result<config::ProjectConfig, proef_core::error::ExitCode> {
    match explicit {
        Some(path) => config::ProjectConfig::load_at(path),
        None => config::ProjectConfig::load(),
    }
    .map_err(|message| {
        crate::render::errln!("error: {message}");
        proef_core::error::ExitCode::UserError
    })
}

/// Like [`load_config`], but a config that was only *discovered* falls back to
/// defaults instead of failing — what `doctor` needs, since it reports on the
/// environment and must run anywhere, including outside a project.
///
/// A config named by `--config` stays fatal even here. The two cases are not
/// the same claim: discovery finding nothing means "no project here", while a
/// named path that is not there is a typo, and answering a typo with a report
/// about some *other* configuration is how `doctor` came to print the error and
/// then run on defaults, exit 0.
///
/// A discovered file that does not *parse* is a third case, and returning the
/// message rather than dropping it is the point of the tuple. Leniency is about
/// a config being **absent**; a `proef.toml` that is sitting right there and
/// broken is the first thing a diagnosis tool should say, and saying nothing
/// made "all checks passed" true only of the configuration doctor had invented.
fn load_config_lenient(
    explicit: Option<&std::path::Path>,
) -> Result<(config::ProjectConfig, Option<String>), proef_core::error::ExitCode> {
    match explicit {
        Some(path) => load_config(Some(path)).map(|config| (config, None)),
        None => Ok(match config::ProjectConfig::load() {
            Ok(config) => (config, None),
            Err(message) => (config::ProjectConfig::default(), Some(message)),
        }),
    }
}

/// Answer for `--config` in a command that reads nothing from it.
///
/// `fmt`, `init` and `schema` take no configuration, and accepted the flag
/// silently — `proef fmt --config /nope.toml` exited 0 while the docs called the
/// flag global to every subcommand. The file is parsed and thrown away: the flag
/// is *checked*, not consumed, so a typo costs the same exit 2 everywhere and
/// discovery is left alone for commands that were never asking about it.
fn check_config_flag(
    explicit: Option<&std::path::Path>,
) -> Result<(), proef_core::error::ExitCode> {
    match explicit {
        None => Ok(()),
        Some(path) => load_config(Some(path)).map(|_| ()),
    }
}

/// The active environment: the `--env` flag wins, else `PROEF_ENV`, else none.
fn active_env(flag: Option<String>) -> Result<Option<String>, proef_core::error::ExitCode> {
    if let Some(flag) = flag {
        return Ok(Some(flag));
    }
    crate::envvar::read("PROEF_ENV").map_err(|message| {
        crate::render::errln!("error: {message}");
        proef_core::error::ExitCode::UserError
    })
}

/// One `--watch` rerun's preamble: read `proef.toml` **again**, then re-resolve
/// the suite from what it now says.
///
/// Separate from the run itself, and named, because the bug it fixes is
/// invisible from the outside: `--watch` watched the config and retriggered on
/// an edit while the rerun still used the snapshot loaded at startup, so
/// changing `[url] base` produced a rerun that dutifully called the old host.
/// Watching a file whose contents you then ignore is worse than not watching it
/// — it reports that the edit was taken.
fn reload_for_rerun(
    explicit: Option<&std::path::Path>,
    typed_path: Option<PathBuf>,
) -> Result<(config::ProjectConfig, PathBuf), proef_core::error::ExitCode> {
    let config = load_config(explicit)?;
    let path = resolve_suite_path(typed_path, &config)?;
    Ok((config, path))
}

/// The shared preamble of every suite command (`test`/`flows`/`artifacts`):
/// load config once, resolve the suite path, and pick the active environment.
///
/// The first two steps *are* [`reload_for_rerun`] — the same pair a `--watch`
/// rerun repeats — so the startup path and the rerun path cannot drift on what
/// "load the config, then resolve the suite against it" means. They were
/// separate copies, which is the shape the `exclusive-tags` bug in this same
/// change came in: two paths that had to agree, and nothing making them.
fn prepare(
    path: Option<PathBuf>,
    env: Option<String>,
    explicit: Option<&std::path::Path>,
) -> Result<(config::ProjectConfig, PathBuf, Option<String>), proef_core::error::ExitCode> {
    let (config, path) = reload_for_rerun(explicit, path)?;
    Ok((config, path, active_env(env)?))
}

/// Output proef could not deliver is an environment failure, whatever the
/// command's own verdict was: a consumer parsing truncated stdout cannot
/// trust the exit code's usual meaning either — a `TestFailure` whose JSON
/// body got cut off mid-write is no more trustworthy than a `Success` that
/// never printed its summary. A stdout failure therefore always wins, never
/// only when the command's own code happened to be `Success`.
fn final_exit(
    code: proef_core::error::ExitCode,
    stdout_failed: bool,
) -> proef_core::error::ExitCode {
    if stdout_failed {
        proef_core::error::ExitCode::SystemError
    } else {
        code
    }
}

// One dispatch table over the CLI surface; splitting arms hides the routing.
#[allow(clippy::too_many_lines)]
fn main() -> std::process::ExitCode {
    render::install();
    // clap renders usage errors itself and exits 2 — which is exactly the
    // user-error contract (ADR-0009); the mapping is pinned by tests/cli.rs.
    let Cli {
        config: config_path,
        command,
    } = Cli::parse();
    let config_path = config_path.as_deref();
    let code = match command {
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
            max_fail,
            shard,
        } => {
            // Captured before `prepare` consumes `path`: `dry_run`'s "next
            // command" nudge must echo the path the user actually typed, not
            // a defaulted one a bare `proef test` already finds on its own.
            let path_given = path.is_some();
            // Kept for `--watch`: each rerun re-resolves the suite from the
            // config it just re-read, and the typed path still has to win.
            let typed_path = path.clone();
            match prepare(path, env, config_path) {
                Err(code) => code,
                // Parse the tag expression once (it is constant across watch reruns);
                // a malformed one is a user error, before any scenario runs.
                Ok((config, path, active_env)) => {
                    match tags.as_deref().map(proef_core::tags::parse).transpose() {
                        Err(message) => {
                            crate::render::errln!("error: {message}");
                            proef_core::error::ExitCode::UserError
                        }
                        Ok(tag_filter) => {
                            // Takes the config rather than closing over one:
                            // under `--watch` the config is re-read per rerun,
                            // and a closure that captured the startup snapshot
                            // is exactly how editing `[url] base` retriggered a
                            // run that still called the old host.
                            let run_once = |config: &config::ProjectConfig, path: &Path, cancel| {
                                if dry_run {
                                    commands::dry_run(
                                        path,
                                        path_given,
                                        tag_filter.as_ref(),
                                        tags.as_deref(),
                                        scenario.as_deref(),
                                        scenario_file.as_deref(),
                                        active_env.as_deref(),
                                        run_id.clone(),
                                        sarif.as_deref(),
                                        config,
                                    )
                                } else {
                                    exec::execute(
                                        path,
                                        tag_filter.as_ref(),
                                        jobs,
                                        output,
                                        junit.as_deref(),
                                        scenario.as_deref(),
                                        scenario_file.as_deref(),
                                        active_env.as_deref(),
                                        run_id.clone(),
                                        rerun,
                                        max_fail,
                                        shard,
                                        config,
                                        cancel, // None = execute installs its own Ctrl-C handler
                                    )
                                }
                            };
                            if watch_mode {
                                // Which *directories* the loop watches is fixed
                                // at startup — rearming a watcher mid-loop
                                // would race the events it is draining — so a
                                // change to `[run] fragments` or `[run] suite`
                                // needs a restart to be watched. What each
                                // rerun *reads* is not: the config is loaded
                                // again below. `runs-dir` is neither, and used
                                // to fall between them: the rerun wrote to the
                                // new directory while the watcher still
                                // excluded the old one, and the output fed the
                                // input. The rerun now registers where it
                                // writes (`RunsDirs`), so the two cannot
                                // disagree.
                                let fragments = config.fragments();
                                let runs_dir = config.runs_dir();
                                // The config this run resolved through, not a
                                // fresh upward search — with `--config` the two
                                // are different files, and watching the wrong
                                // one means edits to the settings driving the
                                // run never retrigger it.
                                let watched = config.path().map(Path::to_path_buf);
                                // **Load-bearing, not tidiness.** Moving the
                                // startup config out here is what makes it
                                // unreachable from the rerun closure below, so
                                // "a rerun must not use the snapshot" is
                                // enforced by the compiler (E0382) rather than
                                // by remembering. Delete this line and the
                                // original bug compiles again.
                                drop(config);
                                watch::watch_loop(
                                    &path,
                                    watched.as_deref(),
                                    fragments.as_deref(),
                                    &runs_dir,
                                    |token, runs_dirs| {
                                        // A config that no longer parses fails
                                        // this rerun and leaves the loop
                                        // watching — the next keystroke may
                                        // well fix it.
                                        match reload_for_rerun(config_path, typed_path.clone()) {
                                            Err(code) => code,
                                            Ok((config, path)) => {
                                                // Before the run writes, not
                                                // after: notify delivers on its
                                                // own thread while the run is
                                                // still going.
                                                runs_dirs.record(&config.runs_dir());
                                                run_once(&config, &path, Some(token))
                                            }
                                        }
                                    },
                                )
                            } else {
                                run_once(&config, &path, None)
                            }
                        }
                    }
                }
            }
        }
        Command::Flows { path, output, env } => match json_only(output) {
            Err(code) => code,
            Ok(output_json) => match prepare(path, env, config_path) {
                Err(code) => code,
                Ok((config, path, active_env)) => {
                    commands::flows(&path, output_json, active_env.as_deref(), &config)
                }
            },
        },
        Command::Fragments {
            path,
            output,
            check,
            require_annotated,
            env,
        } => match json_only(output) {
            Err(code) => code,
            Ok(output_json) => match prepare(path, env, config_path) {
                Err(code) => code,
                Ok((config, path, active_env)) => commands::fragments(
                    &path,
                    output_json,
                    check,
                    require_annotated,
                    active_env.as_deref(),
                    &config,
                ),
            },
        },
        Command::Macros { path, output, env } => match json_only(output) {
            Err(code) => code,
            Ok(output_json) => match prepare(path, env, config_path) {
                Err(code) => code,
                Ok((config, path, active_env)) => {
                    commands::macros(&path, output_json, active_env.as_deref(), &config)
                }
            },
        },
        Command::Artifacts {
            path,
            output,
            run_id,
            env,
        } => match prepare(path, env, config_path) {
            Err(code) => code,
            Ok((config, path, active_env)) => {
                commands::artifacts(&path, &output, run_id, active_env.as_deref(), &config)
            }
        },
        Command::Schema { add_to } => match check_config_flag(config_path) {
            Err(code) => code,
            Ok(()) => commands::schema(&add_to, true),
        },
        Command::Init { dir } => match check_config_flag(config_path) {
            Err(code) => code,
            Ok(()) => init::init(&dir.unwrap_or_else(|| PathBuf::from("."))),
        },
        Command::Doctor => {
            // Lenient about *discovery*, like `proef lsp`: `doctor` reports on
            // the environment and must run anywhere, including outside a
            // project, where no config and no suite simply means there are no
            // packs to check. Never lenient about `--config` — see
            // `load_config_lenient`.
            match load_config_lenient(config_path) {
                Err(code) => code,
                Ok((config, config_error)) => commands::doctor(
                    &registry::engines(),
                    config.default_suite_path().as_deref(),
                    config.fragments().as_deref(),
                    &config.secrets_file(),
                    config_error.as_deref(),
                    &commands::naming(&config),
                ),
            }
        }
        // The store is the *project's*, so `secret` needs the config for the
        // same reason `test` does — and `--config` therefore decides which
        // project's secrets are being listed or written, instead of being
        // accepted and ignored while the store was taken from the shell's cwd.
        Command::Secret { action } => match load_config(config_path) {
            Err(code) => code,
            Ok(config) => {
                let store = config.secrets_file();
                let result = match action {
                    SecretAction::Set { name, stdin } => secretstore::set(&store, &name, stdin),
                    SecretAction::List => secretstore::list(&store),
                    SecretAction::Rm { name } => secretstore::rm(&store, &name),
                };
                match result {
                    Ok(()) => proef_core::error::ExitCode::Success,
                    Err(err) => {
                        crate::render::errln!("error: {}", err.message());
                        // The variant carries the ADR-0009 classification: a typo
                        // exits 2, an unwritable key dir or lock failure exits 3.
                        match err {
                            secretstore::SecretError::User(_) => {
                                proef_core::error::ExitCode::UserError
                            }
                            secretstore::SecretError::System(_) => {
                                proef_core::error::ExitCode::SystemError
                            }
                        }
                    }
                }
            }
        },
        // The three record-reading commands all go through `load_config`,
        // which owns the one spelling of "a malformed proef.toml is a user
        // error" — they each carried their own copy of that rendering. Loud,
        // not lenient: a config that silently defaulted `runs-dir` would
        // misdiagnose "no runs" (same reasoning as `test`, exec.rs).
        Command::Explain { run_id } => match load_config(config_path) {
            Ok(config) => explain::explain(&config.runs_dir(), run_id.as_deref()),
            Err(code) => code,
        },
        Command::Flaky { output } => match json_only(output) {
            Err(code) => code,
            Ok(output_json) => match load_config(config_path) {
                Ok(config) => flaky::flaky(&config.runs_dir(), output_json),
                Err(code) => code,
            },
        },
        Command::Diff {
            base,
            new,
            fail_on_regression,
        } => match load_config(config_path) {
            Ok(config) => diff::diff(
                &config.runs_dir(),
                base.as_deref(),
                new.as_deref(),
                fail_on_regression,
            ),
            Err(code) => code,
        },
        Command::Report { run_id, output } => match load_config(config_path) {
            Ok(config) => report::report(&config.runs_dir(), run_id.as_deref(), output.as_deref()),
            Err(code) => code,
        },
        Command::Fmt { path, check } => match check_config_flag(config_path) {
            Err(code) => code,
            Ok(()) => fmt::fmt(&path, check),
        },
        Command::Lsp => lsp::run(config_path.map(std::path::Path::to_path_buf)),
    };
    let code = final_exit(code, render::stdout_failed());
    std::process::ExitCode::from(code.code())
}

#[cfg(test)]
mod tests {
    // Why: unwrap/expect are acceptable in `#[cfg(test)]` — a broken assumption
    // surfaces as a test failure, which is exactly the intent.
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use proef_core::error::ExitCode;

    // The decision the exit funnel makes, pinned on every platform — the
    // e2e reproduction of the whole path (a real `/dev/full` write failure)
    // only runs on Linux (tests/cli.rs), so this is what keeps the decision
    // itself covered on macOS and Windows too.
    #[test]
    fn a_stdout_failure_upgrades_success_to_system_error() {
        assert_eq!(final_exit(ExitCode::Success, true), ExitCode::SystemError);
    }

    #[test]
    fn a_stdout_failure_upgrades_test_failure_to_system_error() {
        // The command's own verdict is exactly what a stdout failure makes
        // untrustworthy — a JSON body that embeds the verdict and got cut
        // off mid-write is corrupted whether the verdict was pass or fail.
        assert_eq!(
            final_exit(ExitCode::TestFailure, true),
            ExitCode::SystemError
        );
    }

    #[test]
    fn no_stdout_failure_passes_success_through_unchanged() {
        assert_eq!(final_exit(ExitCode::Success, false), ExitCode::Success);
    }

    #[test]
    fn no_stdout_failure_passes_test_failure_through_unchanged() {
        assert_eq!(
            final_exit(ExitCode::TestFailure, false),
            ExitCode::TestFailure
        );
    }

    /// **A `--watch` rerun reads the file, not the snapshot.**
    ///
    /// The regression is silent by construction — the loop prints "change
    /// detected — rerunning" either way, so the only visible symptom is a run
    /// that quietly used the old settings, which is how it survived a release
    /// that claimed to have fixed it. Pinned here rather than by driving a real
    /// watch loop: that would need a background process and sleeps, and the
    /// suite's flake rule is to assert the decision instead of the timing.
    #[test]
    fn a_rerun_sees_an_edited_config_rather_than_the_startup_snapshot() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let config = tmp.path().join("proef.toml");
        std::fs::create_dir(tmp.path().join("features")).expect("suite dir");
        std::fs::write(
            &config,
            "[run]\nsuite = \"features\"\n[url]\nbase = \"one\"\n",
        )
        .expect("write config");

        let first = reload_for_rerun(Some(&config), None).expect("first load");
        assert_eq!(first.0.config_vars(None).expect("vars")["url:base"], "one");

        // The edit that woke the loop.
        std::fs::write(
            &config,
            "[run]\nsuite = \"features\"\n[url]\nbase = \"two\"\n",
        )
        .expect("edit config");

        let second = reload_for_rerun(Some(&config), None).expect("second load");
        assert_eq!(
            second.0.config_vars(None).expect("vars")["url:base"],
            "two",
            "the rerun used the startup snapshot instead of re-reading the file"
        );
        // The suite is re-resolved from the config too, so a rerun after a
        // `[run] suite` edit runs the suite the file now names.
        assert_eq!(second.1, tmp.path().join("features"));
    }

    /// A config that stops parsing mid-session fails *that rerun* and leaves the
    /// loop watching — half-typed TOML is the normal state of a file being
    /// edited, and exiting the watch on it would make `--watch` unusable for the
    /// one file it exists to react to.
    #[test]
    fn a_rerun_over_a_broken_config_is_a_user_error_not_a_panic() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let config = tmp.path().join("proef.toml");
        std::fs::write(&config, "[run\nsuite =").expect("write config");
        assert_eq!(
            reload_for_rerun(Some(&config), None).unwrap_err(),
            ExitCode::UserError
        );
    }

    /// `--config` names a file; a named file that is not there is a typo, and
    /// the answer is the same exit 2 whether or not the command reads anything
    /// from it. `fmt`, `init` and `schema` used to accept it and exit 0.
    #[test]
    fn a_named_config_that_is_missing_is_refused_even_where_nothing_reads_it() {
        let missing = std::path::Path::new("definitely/not/here/proef.toml");
        assert_eq!(
            check_config_flag(Some(missing)).unwrap_err(),
            ExitCode::UserError
        );
        // No flag, no claim to check — discovery stays the lenient path.
        assert!(check_config_flag(None).is_ok());
    }

    /// `doctor` must run anywhere, including outside a project — but "no config
    /// found" and "the config you named is not there" are different claims, and
    /// collapsing them is how `doctor --config /nope.toml` printed the error and
    /// then reported on defaults, exit 0.
    #[test]
    fn doctor_is_lenient_about_discovery_and_strict_about_the_flag() {
        assert!(
            load_config_lenient(None).is_ok(),
            "discovery finding nothing must not stop doctor"
        );
        assert_eq!(
            load_config_lenient(Some(std::path::Path::new("no/such/proef.toml"))).unwrap_err(),
            ExitCode::UserError
        );
    }
}
