//! Cooperative cancellation plumbing (ADR-0007).
//!
//! Cancellation is cooperative at **batch boundaries**: the orchestrator checks the
//! token before opening sessions and before each batch; engines receive it through
//! [`crate::engine::EngineSession::run_batch`] and may honor it at finer grain when
//! they can. The token is re-exported here so engines and the CLI share one type
//! without depending on `tokio-util` directly — it is runtime-agnostic (no tokio
//! runtime enters the dependency tree; `default-features = false`).

pub use tokio_util::sync::CancellationToken;

#[cfg(test)]
mod tests {
    use super::CancellationToken;

    /// Polling and child tokens work from plain threads (ADR-0007 verified fact —
    /// kept as a pinned regression against tokio-util changes).
    #[test]
    fn token_polls_and_propagates_to_children_without_a_runtime() {
        let root = CancellationToken::new();
        let child = root.child_token();
        assert!(!child.is_cancelled());
        root.cancel();
        assert!(child.is_cancelled());
    }
}
