//! One wholesale recompute of the whole suite. The suite is tens of small files
//! and the pipeline is milliseconds, so recomputing everything on every change
//! is cheaper than maintaining an incremental index.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use proef_core::analyze::{AnalyzeCtx, SuiteAnalysis, analyze_suite};
use proef_core::engine::StepKindSpec;
use proef_core::pack::{FragmentCorpus, PackSource};
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
    /// The fragment corpus, held by the caller across recomputes.
    pub fragments: &'a FragmentCorpus,
}

/// Read the whole fragment corpus through the overlay.
///
/// Through the overlay, not the disk, so an **unsaved** edit to a `.hurl` file
/// is what analysis sees — the same rule packs and features already follow.
///
/// Called when a fragment file changes, never per request. A corpus rebuilt per
/// request also rebuilt its scan memo, so the LSP re-read and re-hurl-parsed
/// every file in it on each completion, definition and debounce tick; on a
/// corpus of any size that is the dominant cost of typing.
pub fn read_fragments(
    docs: &Documents,
    disk: &dyn SourceProvider,
    kinds: &[StepKindSpec],
) -> FragmentCorpus {
    let overlay = OverlaySourceProvider::new(docs, disk);
    let mut sources = Vec::new();
    let mut errors = Vec::new();
    // The same core decision the CLI applies (`CorpusBudget`), for a sharper
    // reason here: this corpus is *held* between requests, so an oversized
    // file is resident for the whole editing session rather than for one
    // command. Only the measurement is ours — text length after the read,
    // because a source may be an unsaved buffer with no file to stat; the
    // allocation is the editor's either way, and what this prevents is
    // retaining it.
    let mut budget = proef_core::pack::CorpusBudget::new();
    for name in overlay.discover_fragments().unwrap_or_default() {
        match overlay.read(&name) {
            Ok(text) => match budget.admit(&name, text.len() as u64) {
                proef_core::pack::Admit::Skip(diag) => {
                    errors.push(diag.with_source(name, Arc::from("")));
                }
                proef_core::pack::Admit::Stop(diag) => {
                    errors.push(diag.with_source(name, Arc::from("")));
                    break;
                }
                proef_core::pack::Admit::Read => sources.push(PackSource { name, text }),
            },
            // A corpus is foreign by design: one unreadable file reports itself
            // and the rest still load.
            // The same diagnostic the CLI reports, help text included — it is
            // the corpus's own statement about an unreadable file, not the
            // reader's. `with_source` on top so the editor can position it.
            Err(err) => errors.push(
                FragmentCorpus::unreadable_file(&name, &err.0).with_source(name, Arc::from("")),
            ),
        }
    }
    FragmentCorpus::new(sources, kinds).with_read_errors(errors)
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
        fragments: inputs.fragments,
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

    // Fragment files too: go-to-definition into one needs its text to turn a
    // byte span into a range, and a fragment file with no diagnostic would
    // otherwise never be captured above. The analysis already holds the exact
    // bytes it measured the spans against, so this takes them rather than
    // re-reading — no second read per fragment per recompute, and no window in
    // which a span is converted against text it was not computed from.
    for f in &suite.fragments {
        raw.entry(f.file.clone())
            .or_insert_with(|| Arc::clone(&f.source));
    }

    Analysis { suite, raw }
}
