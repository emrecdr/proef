//! Engine registry assembly — one line per engine, cargo-feature-gated (ADR-0002).
//!
//! Adding an engine to proef is: a new crate implementing the seam traits, one
//! optional dependency, and one line here. Nothing in `proef-core` changes — that
//! is the structural acceptance test.

use std::collections::BTreeMap;

use proef_core::engine::{EngineFactory, StepKindSpec};

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

/// Every step kind the compiled-in engines contribute, in registration order.
///
/// The one place that assembles them, so pack validation, discovery, `--watch`
/// and the LSP cannot end up asking a differently-built registry (ADR-0002).
pub fn step_kinds() -> Vec<StepKindSpec> {
    engines()
        .iter()
        .flat_map(|engine| engine.step_kinds().iter().copied())
        .collect()
}

/// Step-kind prefix → engine id: the lowering routing table (ADR-0002).
///
/// Assembled here for the same reason as [`step_kinds`], and beside it: the two
/// are always built together from the same `engines()` walk, so a caller that
/// hand-rolled one and called the other could route steps to an engine whose
/// kinds it never asked for.
pub fn kind_to_engine() -> BTreeMap<String, String> {
    engines()
        .iter()
        .flat_map(|engine| {
            engine
                .step_kinds()
                .iter()
                .map(|kind| (kind.prefix.to_owned(), engine.id().to_owned()))
        })
        .collect()
}
