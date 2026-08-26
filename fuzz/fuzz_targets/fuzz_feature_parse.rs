//! Feature-file parser totality: arbitrary Gherkin bytes must produce a
//! `FeatureFile` or diagnostics, never a panic (TESTING-STRATEGY).
//!
//! The last user-input parser without a target. Every diagnostic it emits
//! carries a byte span into the *normalized* source (BOM stripped, trailing
//! newline appended), and those spans are consumed downstream by miette and by
//! the LSP's UTF-16 converter — so a span that is out of range or lands
//! mid-codepoint is a panic somewhere else entirely. Checking them here is
//! what makes this target worth more than "did not crash".
#![no_main]

use libfuzzer_sys::fuzz_target;
use proef_core::feature;

/// The normalization `feature::parse` applies before measuring spans, mirrored
/// so a span can be checked against the text it actually indexes.
fn normalized(text: &str) -> String {
    let mut out = text.strip_prefix('\u{feff}').unwrap_or(text).to_owned();
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let source = normalized(text);
    match feature::parse("fuzz.feature", text) {
        Ok(file) => {
            // Every scenario's step spans must index the normalized source.
            for scenario in &file.scenarios {
                for step in &scenario.steps {
                    assert!(
                        step.span.start <= step.span.end && step.span.end <= source.len(),
                        "step span {:?} out of range for {} bytes",
                        step.span,
                        source.len()
                    );
                    assert!(
                        source.is_char_boundary(step.span.start)
                            && source.is_char_boundary(step.span.end),
                        "step span {:?} splits a codepoint",
                        step.span
                    );
                }
            }
        }
        Err(diags) => {
            for diag in &diags {
                let Some(span) = diag.span else { continue };
                // A diagnostic's span indexes the source it carries, which is
                // the normalized text.
                let len = diag.source_text.as_ref().map_or(source.len(), |t| t.len());
                assert!(
                    span.start <= span.end && span.end <= len,
                    "diag {} span {span:?} out of range for {len} bytes",
                    diag.code
                );
                if let Some(text) = diag.source_text.as_ref() {
                    assert!(
                        text.is_char_boundary(span.start) && text.is_char_boundary(span.end),
                        "diag {} span {span:?} splits a codepoint",
                        diag.code
                    );
                }
            }
        }
    }
});
