//! One wholesale recompute of the whole suite. The suite is tens of small files
//! and the pipeline is milliseconds, so recomputing everything on every change
//! is cheaper than maintaining an incremental index.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use proef_core::analyze::{AnalyzeCtx, SuiteAnalysis, analyze_suite};
use proef_core::engine::StepKindSpec;
use proef_core::provider::SourceProvider;

use crate::documents::{Documents, OverlaySourceProvider};

/// A cached recompute plus the raw text of each analyzed source, needed to
/// build a [`crate::convert::LineIndex`] when converting spans for that file.
pub struct Analysis {
    /// The whole-suite analysis: diagnostics, bindings, and macro definitions.
    pub suite: SuiteAnalysis,
    /// The raw editor text of every source the analysis touched, keyed by name.
    pub raw: BTreeMap<String, Arc<str>>,
}

/// The injected inputs of one recompute. Borrowed from the server config and the
/// live document overlay so a recompute allocates nothing beyond its result.
pub struct RecomputeInputs<'a> {
    /// The suite root the disk provider walks; discovery keys off it.
    pub root: &'a PathBuf,
    /// The open-buffer overlay (unsaved edits win over disk bytes).
    pub docs: &'a Documents,
    /// The disk fallback behind the overlay.
    pub disk: &'a dyn SourceProvider,
    /// Registered engine step kinds (drives pack validation).
    pub kinds: &'a [StepKindSpec],
    /// Step-kind prefix → engine id, the lowering routing table.
    pub kind_to_engine: &'a BTreeMap<String, String>,
    /// Injected environment snapshot (`${env:…}`).
    pub env: &'a BTreeMap<String, String>,
    /// Injected `proef.toml` config scope (`${url:…}` / `${vars:…}`).
    pub config_vars: &'a BTreeMap<String, String>,
}

/// Read every pack and feature through the overlay-then-disk provider and run the
/// collect-all suite analysis, then capture the raw text of every source it
/// touched so span conversion has an index to build against.
pub fn recompute(inputs: &RecomputeInputs<'_>) -> Analysis {
    // The root is the disk provider's own concern; discovery keys off it there.
    let _ = inputs.root;

    let overlay = OverlaySourceProvider::new(inputs.docs, inputs.disk);

    let suite = analyze_suite(&AnalyzeCtx {
        provider: &overlay,
        kinds: inputs.kinds,
        kind_to_engine: inputs.kind_to_engine,
        env: inputs.env,
        config_vars: inputs.config_vars,
        run_id: "lsp",
    });

    // Capture the raw text of every source the analysis touched, so features can
    // build a converter for it. Re-reading is cheap (overlay/disk, tens of files).
    let mut raw = BTreeMap::new();
    for name in suite.diagnostics.keys().cloned().collect::<Vec<_>>() {
        if let Ok(text) = overlay.read(&name) {
            raw.insert(name, text);
        }
    }
    for b in &suite.bindings {
        if !raw.contains_key(&b.feature)
            && let Ok(text) = overlay.read(&b.feature)
        {
            raw.insert(b.feature.clone(), text);
        }
    }
    for m in &suite.macros {
        if !raw.contains_key(&m.pack)
            && let Ok(text) = overlay.read(&m.pack)
        {
            raw.insert(m.pack.clone(), text);
        }
    }

    Analysis { suite, raw }
}
