//! `--watch`: rerun on feature/pack changes via `notify` (pinned 8.2.0).

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
/// `runs_dir` covers a `[run] runs-dir` that is not a dot-directory and so is
/// not already skipped.
fn is_authored(
    path: &Path,
    watched_exts: &[&str],
    config_path: Option<&Path>,
    runs_dir: Option<&str>,
) -> bool {
    // The config is matched by exact path, not by extension: a `.toml`
    // allowlist would requeue on any unrelated manifest in the tree.
    if config_path.is_some_and(|c| path == c) {
        return true;
    }
    // Every component of the parent is a directory this path sits under. Taken
    // via `parent` rather than by collecting and popping: this runs per
    // filesystem event, and dropping the file name does not need an allocation.
    for component in path.parent().into_iter().flat_map(Path::components) {
        let dir = Path::new(component.as_os_str());
        if crate::front::skipped_dir(dir)
            || runs_dir.is_some_and(|name| component.as_os_str() == std::ffi::OsStr::new(name))
        {
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
pub fn watch_loop(
    path: &Path,
    config_path: Option<&Path>,
    fragments: Option<&Path>,
    runs_dir: &Path,
    mut once: impl FnMut(CancellationToken) -> ExitCode,
) -> ExitCode {
    // `proef.toml` lives above the suite, so the recursive watch below never
    // sees it — and `[url]`/`[vars]`/`[env.*]` decide what every scenario
    // resolves to. Editing a base URL and watching nothing happen reads as the
    // watcher being broken.
    let config_path = config_path.map(Path::to_path_buf);
    let watched_config = config_path.clone();
    let watched_exts = watched_extensions();
    // The last path component of the configured runs dir. Matched by name, so a
    // root spelled differently by the watcher than by the config still matches.
    let watched_runs_dir = runs_dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned());
    let (tx, rx) = mpsc::channel::<()>();
    let mut watcher = match notify::recommended_watcher(move |event| {
        if let Ok(event) = event {
            let event: notify::Event = event;
            if !(event.kind.is_modify() || event.kind.is_create() || event.kind.is_remove()) {
                return;
            }
            let authored = event.paths.iter().any(|p| {
                is_authored(
                    p,
                    &watched_exts,
                    watched_config.as_deref(),
                    watched_runs_dir.as_deref(),
                )
            });
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
        let code = once(token);
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
    use super::*;
    use std::path::PathBuf;

    const EXTS: &[&str] = &["feature", "yaml", "yml", "hurl"];

    fn authored(path: &str, runs_dir: Option<&str>) -> bool {
        is_authored(&PathBuf::from(path), EXTS, None, runs_dir)
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

    /// The config is matched by exact path: a `.toml` extension rule would
    /// requeue on any unrelated manifest in the tree.
    #[test]
    fn the_config_matches_by_path_not_extension() {
        let config = PathBuf::from("/proj/proef.toml");
        assert!(is_authored(&config, EXTS, Some(&config), None));
        assert!(!is_authored(
            &PathBuf::from("/proj/Cargo.toml"),
            EXTS,
            Some(&config),
            None
        ));
    }
}
