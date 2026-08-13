//! `--watch`: rerun on feature/pack changes via `notify` (pinned 8.2.0).

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use notify::{RecursiveMode, Watcher};
use proef_core::cancel::CancellationToken;
use proef_core::error::ExitCode;

/// Extensions whose edits retrigger a run: proef's own authored formats, plus
/// whatever the registered engines claim for fragment files.
///
/// The engine half is **asked of the registry**, never hardcoded. Discovery
/// already derives it from `StepKindSpec::fragments`, and a second engine whose
/// fragment edits silently failed to retrigger `--watch` is exactly the drift
/// ADR-0002 exists to prevent. Anything else — run records, the state file,
/// editor swap files — must not requeue, or a watched tree containing proef's
/// own output reruns itself forever.
fn watched_extensions() -> Vec<&'static str> {
    let kinds = crate::registry::step_kinds();
    let mut exts = crate::front::authored_extensions();
    exts.extend(crate::front::fragment_extensions(&kinds));
    exts
}

/// Every directory this session has written run records to, by **name**.
///
/// Shared and accumulating rather than a single name fixed at startup, because
/// a rerun re-reads the config — that is the whole point of watching it — so
/// `[run] runs-dir` can change mid-loop. When it did, records went to the *new*
/// directory while the exclusion still named the *old* one, and each rerun's
/// `artifacts/*.hurl` under the now-unexcluded directory requeued the next run:
/// 39 runs in 12 seconds, firing real traffic, from one edit.
///
/// The fix is to stop having two answers to "where does proef write" rather
/// than to keep them in step. The rerun registers its directory before it
/// writes there, so the exclusion is derived from the same config the run is.
///
/// Never emptied. Events from the directory a *previous* run wrote can still be
/// in flight when the name changes, and a stale exclusion costs nothing — it
/// only means proef declines to retrigger on a directory it used to own.
#[derive(Clone, Default)]
pub(crate) struct RunsDirs(Arc<Mutex<BTreeSet<String>>>);

impl RunsDirs {
    /// Record where the next run will write, **before** it writes there.
    ///
    /// Ordering is load-bearing: `notify` delivers events on its own thread
    /// while the run is still going, so a directory registered afterwards has
    /// already fed the queue.
    pub(crate) fn record(&self, dir: &Path) {
        let Some(name) = dir.file_name() else { return };
        if let Ok(mut seen) = self.0.lock() {
            seen.insert(name.to_string_lossy().into_owned());
        }
    }

    /// Whether `component` names one of them. By name, not by path prefix, for
    /// the reason [`is_authored`] gives.
    fn holds(&self, component: &OsStr) -> bool {
        self.0.lock().is_ok_and(|seen| {
            seen.iter()
                .any(|name| OsStr::new(name.as_str()) == component)
        })
    }
}

/// Whether `path` and `config` are the same file.
///
/// Exact comparison first, then canonical. The fallback is what makes a
/// relatively-typed or symlinked `--config` work at all: `notify` reports
/// events under the spelling the OS resolved them to (`/private/var/…` on
/// macOS), which never equals `/var/…` however the flag was written. Absolute
/// is not enough — `std::path::absolute` is lexical and leaves both the alias
/// and the symlink in place.
///
/// Canonicalising costs a syscall, so it is reached only once the file *names*
/// match — in practice only when a `proef.toml` is actually touched. The
/// canonical form is compared and then dropped: never stored, never printed,
/// which is what keeps Windows's `\\?\` UNC spelling out of every path proef
/// shows a user or hands to another program.
fn same_file(path: &Path, config: &Path) -> bool {
    if path == config {
        return true;
    }
    if path.file_name() != config.file_name() {
        return false;
    }
    // Either side may be mid-rename (an editor's atomic save) and so not exist
    // for an instant. A path that cannot be resolved simply is not a match.
    match (path.canonicalize(), config.canonicalize()) {
        (Ok(event), Ok(watched)) => event == watched,
        _ => false,
    }
}

/// Whether a changed path is an **authored input** — the one decision the
/// watcher makes, extracted so it can be tested without a filesystem.
///
/// An extension allowlist alone is not enough, and assuming it was is what broke
/// this: the allowlist gained the engines' fragment extensions, `.hurl` among
/// them, while every run writes `.proef-runs/<id>/artifacts/*.hurl`. Watching a
/// tree that contained its own runs dir then fed itself — 49 runs in 15 seconds,
/// against a real API, in a tight loop.
///
/// So generated trees are excluded **by directory name**, reusing discovery's
/// own [`crate::front::skipped_dir`]: one rule, two consumers, no second list to
/// drift. By name rather than by path prefix on purpose — a prefix comparison
/// would need both sides canonicalized to survive macOS's `/var` →
/// `/private/var` aliasing, and would silently stop matching when it did not.
/// [`RunsDirs`] covers a `[run] runs-dir` that is not a dot-directory and so is
/// not already skipped.
fn is_authored(
    path: &Path,
    watched_exts: &[&str],
    config_path: Option<&Path>,
    runs: &RunsDirs,
) -> bool {
    // The config is matched by identity, not by extension: a `.toml` allowlist
    // would requeue on any unrelated manifest in the tree.
    if config_path.is_some_and(|c| same_file(path, c)) {
        return true;
    }
    // Every component of the parent is a directory this path sits under. Taken
    // via `parent` rather than by collecting and popping: this runs per
    // filesystem event, and dropping the file name does not need an allocation.
    for component in path.parent().into_iter().flat_map(Path::components) {
        let dir = Path::new(component.as_os_str());
        if crate::front::skipped_dir(dir) || runs.holds(component.as_os_str()) {
            return false;
        }
    }
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| watched_exts.iter().any(|ext| e.eq_ignore_ascii_case(ext)))
}

/// Run `once` immediately, then again after every filesystem change under
/// `path` (debounced). The loop owns the Ctrl-C lifecycle (ADR-0007): the
/// first interrupt cancels the in-flight run gracefully and leaves the loop
/// once it drains; a second interrupt hard-exits.
///
/// `once` is handed the [`RunsDirs`] registry and **must** record the directory
/// its run is about to write to. Passing it in rather than deriving it here is
/// what keeps a single answer to "where does proef write": the rerun already
/// loaded the config, and asking it to say so costs one call, where re-deriving
/// it here would reload the file and could disagree.
pub fn watch_loop(
    path: &Path,
    config_path: Option<&Path>,
    fragments: Option<&Path>,
    runs_dir: &Path,
    mut once: impl FnMut(CancellationToken, &RunsDirs) -> ExitCode,
) -> ExitCode {
    // `proef.toml` lives above the suite, so the recursive watch below never
    // sees it — and `[url]`/`[vars]`/`[env.*]` decide what every scenario
    // resolves to. Editing a base URL and watching nothing happen reads as the
    // watcher being broken.
    let config_path = config_path.map(Path::to_path_buf);
    let watched_config = config_path.clone();
    let watched_exts = watched_extensions();
    // Seeded with the startup config's runs dir, so the very first run — which
    // happens before any rerun can register anything — is excluded too.
    let runs = RunsDirs::default();
    runs.record(runs_dir);
    let watched_runs = runs.clone();
    let (tx, rx) = mpsc::channel::<()>();
    let mut watcher = match notify::recommended_watcher(move |event| {
        if let Ok(event) = event {
            let event: notify::Event = event;
            if !(event.kind.is_modify() || event.kind.is_create() || event.kind.is_remove()) {
                return;
            }
            let authored = event
                .paths
                .iter()
                .any(|p| is_authored(p, &watched_exts, watched_config.as_deref(), &watched_runs));
            if authored {
                let _ = tx.send(());
            }
        }
    }) {
        Ok(watcher) => watcher,
        Err(err) => {
            crate::render::errln!("error: cannot start watcher: {err}");
            return ExitCode::SystemError;
        }
    };
    if let Err(err) = watcher.watch(path, RecursiveMode::Recursive) {
        crate::render::errln!("error: cannot watch {}: {err}", path.display());
        return ExitCode::SystemError;
    }
    // A fragment root may sit outside the suite (a corpus in another repo), so
    // the recursive suite watch above does not necessarily cover it. Same
    // warn-do-not-fail rule as the config: the suite watch is the primary one.
    if let Some(fragments) = fragments
        && let Err(err) = watcher.watch(fragments, RecursiveMode::Recursive)
    {
        crate::render::errln!(
            "warning: cannot watch {} (fragment edits will not retrigger): {err}",
            fragments.display()
        );
    }

    // Watched as a single file, not recursively: its directory is the project
    // root, which may hold anything. A config that cannot be watched is a
    // warning, not a failure — the suite watch above is the primary one.
    if let Some(config_path) = config_path.as_deref()
        && let Err(err) = watcher.watch(config_path, RecursiveMode::NonRecursive)
    {
        crate::render::errln!(
            "warning: cannot watch {} (config edits will not retrigger): {err}",
            config_path.display()
        );
    }

    // One handler for the whole loop; it cancels whichever run is current.
    let stop = Arc::new(AtomicBool::new(false));
    let current: Arc<Mutex<CancellationToken>> = Arc::new(Mutex::new(CancellationToken::new()));
    {
        let stop = Arc::clone(&stop);
        let current = Arc::clone(&current);
        let pressed = AtomicBool::new(false);
        let _ = ctrlc::set_handler(move || {
            if pressed.swap(true, Ordering::SeqCst) {
                crate::render::errln!("\nsecond interrupt — hard exit");
                std::process::exit(crate::INTERRUPT_EXIT_CODE);
            }
            crate::render::errln!(
                "\n[watch] interrupt — cancelling the current run, leaving watch (Ctrl-C again to force)"
            );
            stop.store(true, Ordering::SeqCst);
            if let Ok(token) = current.lock() {
                token.cancel();
            }
        });
    }

    loop {
        let token = CancellationToken::new();
        if let Ok(mut guard) = current.lock() {
            *guard = token.clone();
        }
        let code = once(token, &runs);
        if stop.load(Ordering::SeqCst) {
            return code;
        }
        crate::render::errln!(
            "\n[watch] run finished (exit {}) — watching {} for changes (Ctrl-C to stop)",
            code.code(),
            path.display()
        );
        // Wait for a change (polling so an interrupt can end the wait), then
        // debounce the burst.
        loop {
            if stop.load(Ordering::SeqCst) {
                return code;
            }
            match rx.recv_timeout(Duration::from_millis(200)) {
                Ok(()) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return code,
            }
        }
        while rx.recv_timeout(Duration::from_millis(300)).is_ok() {}
        crate::render::errln!("[watch] change detected — rerunning\n");
    }
}

#[cfg(test)]
mod tests {
    // `same_file` answers a question only the filesystem can, so its test writes
    // real files; the rest of this module stays filesystem-free.
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use std::path::PathBuf;

    const EXTS: &[&str] = &["feature", "yaml", "yml", "hurl"];

    /// A registry holding exactly the named run directories.
    fn dirs(names: &[&str]) -> RunsDirs {
        let runs = RunsDirs::default();
        for name in names {
            runs.record(Path::new(name));
        }
        runs
    }

    fn authored(path: &str, runs_dir: Option<&str>) -> bool {
        let runs = runs_dir.map_or_else(RunsDirs::default, |name| dirs(&[name]));
        is_authored(&PathBuf::from(path), EXTS, None, &runs)
    }

    /// The regression this filter exists for. `--watch` writes
    /// `.proef-runs/<id>/artifacts/*.hurl`, and the extension allowlist gained
    /// `.hurl` for fragment corpora — so a tree containing its own runs dir fed
    /// itself, 49 runs in 15 seconds, firing real traffic in a loop.
    #[test]
    fn a_run_of_its_own_output_never_retriggers() {
        assert!(
            !authored(".proef-runs/019f/artifacts/a--s.hurl", Some(".proef-runs")),
            "an emitted artifact must not requeue the run that wrote it"
        );
        assert!(
            !authored(".proef-runs/019f/events.jsonl", Some(".proef-runs")),
            "nor any other run record"
        );
        // A custom, non-dot runs dir is not covered by the dot rule, which is
        // the whole reason the name is passed in rather than assumed.
        assert!(!authored("out/019f/artifacts/a--s.hurl", Some("out")));
    }

    /// …and the inputs still do, or the filter would have fixed the loop by
    /// breaking the feature.
    #[test]
    fn authored_inputs_still_retrigger() {
        assert!(authored("tests/features/a.feature", Some(".proef-runs")));
        assert!(authored(
            "tests/features/packs/api.yaml",
            Some(".proef-runs")
        ));
        assert!(
            authored("tests/hurl/admin.hurl", Some(".proef-runs")),
            "a fragment corpus edit is exactly what the .hurl extension was added for"
        );
        assert!(authored("suite/UPPER.FEATURE", Some(".proef-runs")));
    }

    /// The exclusion mirrors discovery's own skip list rather than adding a
    /// second one — a `target/` full of build output must not requeue either.
    #[test]
    fn generated_trees_are_skipped_the_way_discovery_skips_them() {
        assert!(!authored("target/debug/build/x/out.yaml", None));
        assert!(!authored(".git/MERGE_MSG.feature", None));
        assert!(!authored("node_modules/pkg/a.yaml", None));
    }

    /// The config is matched by identity: a `.toml` extension rule would
    /// requeue on any unrelated manifest in the tree.
    #[test]
    fn the_config_matches_by_path_not_extension() {
        let config = PathBuf::from("/proj/proef.toml");
        let none = RunsDirs::default();
        assert!(is_authored(&config, EXTS, Some(&config), &none));
        assert!(!is_authored(
            &PathBuf::from("/proj/Cargo.toml"),
            EXTS,
            Some(&config),
            &none
        ));
        // Same file name, different directory — the name pre-check must not be
        // mistaken for the match itself.
        assert!(!is_authored(
            &PathBuf::from("/other/proef.toml"),
            EXTS,
            Some(&config),
            &none
        ));
    }

    /// A relatively-typed or symlinked `--config` still matches the absolute
    /// path `notify` reports. This is the comparison that silently failed:
    /// `--watch --config proef.toml` produced zero retriggers on a config edit
    /// while feature edits kept working, so the loop looked alive.
    #[test]
    fn the_config_matches_through_an_alias_not_just_an_exact_path() {
        let dir = std::env::temp_dir().join("proef-watch-alias-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let config = dir.join("proef.toml");
        std::fs::write(&config, "[run]\n").unwrap();

        // What `notify` delivers: the OS's own resolved spelling.
        let resolved = config.canonicalize().unwrap();
        assert!(
            same_file(&resolved, &config),
            "the watched spelling and the reported spelling are one file"
        );
        // The relative form, resolved against a cwd that is already canonical.
        let absolute = std::path::absolute(&config).unwrap();
        assert!(same_file(&resolved, &absolute));
        // A different file of the same name is still not a match.
        let other = dir.join("nested");
        std::fs::create_dir_all(&other).unwrap();
        let decoy = other.join("proef.toml");
        std::fs::write(&decoy, "[run]\n").unwrap();
        assert!(!same_file(&resolved, &decoy));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The runaway-loop regression. A rerun re-reads the config, so
    /// `[run] runs-dir` can change mid-watch; when the exclusion stayed frozen
    /// at the startup name, the new directory's `artifacts/*.hurl` requeued the
    /// next run — 39 runs in 12 seconds. Both directories must stay excluded.
    #[test]
    fn a_runs_dir_changed_mid_watch_still_never_retriggers() {
        let runs = dirs(&["out1"]);
        assert!(!is_authored(
            &PathBuf::from("out1/019f/artifacts/a--s.hurl"),
            EXTS,
            None,
            &runs
        ));
        // The rerun registers where it is about to write.
        runs.record(Path::new("out2"));
        assert!(
            !is_authored(
                &PathBuf::from("out2/019f/artifacts/a--s.hurl"),
                EXTS,
                None,
                &runs
            ),
            "the directory the reloaded config writes to must not feed the loop"
        );
        // …and the one it used to write to stays excluded, because its events
        // can still be in flight.
        assert!(!is_authored(
            &PathBuf::from("out1/019f/artifacts/a--s.hurl"),
            EXTS,
            None,
            &runs
        ));
        // A real input under neither is unaffected.
        assert!(is_authored(
            &PathBuf::from("tests/features/a.feature"),
            EXTS,
            None,
            &runs
        ));
    }

    /// A run directory named by `--run-id` is not uuid-shaped, which is why the
    /// exclusion is the configured directory rather than a shape heuristic.
    #[test]
    fn a_pinned_run_id_is_excluded_like_any_other_run() {
        let runs = dirs(&["out"]);
        assert!(!is_authored(
            &PathBuf::from("out/ci/artifacts/a--s.hurl"),
            EXTS,
            None,
            &runs
        ));
    }
}
