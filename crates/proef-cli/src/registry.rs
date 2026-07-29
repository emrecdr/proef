//! Engine registry assembly — one line per engine, cargo-feature-gated (ADR-0002).
//!
//! Adding an engine to proef is: a new crate implementing the seam traits, one
//! optional dependency, and one line here. Nothing in `proef-core` changes — that
//! is the structural acceptance test.

use proef_core::engine::EngineFactory;

/// All engines compiled into this binary, in registration order.
// One cfg-gated `push` per engine is the registry's whole point; a `vec![]`
// literal cannot carry per-element cfg attributes.
#[allow(clippy::vec_init_then_push)]
pub fn engines() -> Vec<Box<dyn EngineFactory>> {
    #[allow(unused_mut)]
    let mut engines: Vec<Box<dyn EngineFactory>> = Vec::new();
    #[cfg(feature = "engine-hurl")]
    engines.push(Box::new(proef_engine_hurl::HurlEngineFactory));
    engines
}
