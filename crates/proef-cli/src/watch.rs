//! `--watch`: rerun on feature/pack changes via `notify` (pinned 8.2.0).

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use notify::{RecursiveMode, Watcher};
use proef_core::cancel::CancellationToken;
use proef_core::error::ExitCode;

/// Run `once` immediately, then again after every filesystem change under
/// `path` (debounced). The loop owns the Ctrl-C lifecycle (ADR-0007): the
/// first interrupt cancels the in-flight run gracefully and leaves the loop
/// once it drains; a second interrupt hard-exits.
pub fn watch_loop(path: &Path, mut once: impl FnMut(CancellationToken) -> ExitCode) -> ExitCode {
    let (tx, rx) = mpsc::channel::<()>();
    let mut watcher = match notify::recommended_watcher(move |event| {
        if let Ok(event) = event {
            let event: notify::Event = event;
            if !(event.kind.is_modify() || event.kind.is_create() || event.kind.is_remove()) {
                return;
            }
            // Only authored inputs retrigger: feature files and pack YAML.
            // Anything else — run records under the runs dir, the state file,
            // editor swap files — must not requeue, or a watched tree that
            // contains proef's own output reruns itself forever.
            let authored = event.paths.iter().any(|p| {
                p.extension().and_then(|e| e.to_str()).is_some_and(|e| {
                    e.eq_ignore_ascii_case("feature")
                        || e.eq_ignore_ascii_case("yaml")
                        || e.eq_ignore_ascii_case("yml")
                })
            });
            if authored {
                let _ = tx.send(());
            }
        }
    }) {
        Ok(watcher) => watcher,
        Err(err) => {
            eprintln!("error: cannot start watcher: {err}");
            return ExitCode::SystemError;
        }
    };
    if let Err(err) = watcher.watch(path, RecursiveMode::Recursive) {
        eprintln!("error: cannot watch {}: {err}", path.display());
        return ExitCode::SystemError;
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
                eprintln!("\nsecond interrupt — hard exit");
                std::process::exit(crate::INTERRUPT_EXIT_CODE);
            }
            eprintln!(
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
        eprintln!(
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
        eprintln!("[watch] change detected — rerunning\n");
    }
}
