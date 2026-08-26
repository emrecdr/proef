//! Source-located diagnostics with stable codes (ADR-0009, TECH-SPEC §9).
//!
//! The core produces structured [`Diag`]s; **only `proef-cli` renders them**
//! (miette stays out of library crates). Every diagnostic carries a stable,
//! greppable code (`proef::pack::adjacent_captures`, …) — the seeded error
//! corpus names one file per code (TESTING-STRATEGY §4).
//!
//! Spans are 0-based **byte** offsets, end-exclusive — directly convertible to
//! miette's `SourceSpan`. Gherkin span caveats (trailing-newline normalization,
//! char-counted `LineCol`) are handled where spans are produced, never here.

use std::sync::Arc;

/// A byte span into a source text (0-based, end-exclusive).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    /// First byte of the region.
    pub start: usize,
    /// One past the last byte of the region.
    pub end: usize,
}

impl Span {
    /// A span clamped into `len` (guards against parser spans past a
    /// normalized/appended trailing newline).
    pub fn clamped(start: usize, end: usize, len: usize) -> Self {
        let start = start.min(len);
        Self {
            start,
            end: end.clamp(start, len),
        }
    }

    /// Length in bytes. Saturating: the fields are public, so an inverted
    /// span is constructible without [`Span::clamped`] — B1's shipped class —
    /// and a raw subtraction turned that authoring slip into a panic (debug)
    /// or a near-`usize::MAX` length fed to miette (release).
    pub fn len(&self) -> usize {
        self.end.saturating_sub(self.start)
    }

    /// Whether the span is empty.
    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }
}

/// Diagnostic severity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// Blocks the run (exit 2 — user fault).
    Error,
    /// Surfaced, does not block.
    Warning,
}

/// One structured, source-located finding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diag {
    /// Stable, greppable code (`proef::pack::…`, `proef::feature::…`, …).
    pub code: &'static str,
    /// Severity.
    pub severity: Severity,
    /// Human-readable message.
    pub message: String,
    /// Name of the source this points into (file path as authored), if any.
    pub source_name: Option<String>,
    /// The (normalized) source text, shared across diags of one file.
    pub source_text: Option<Arc<str>>,
    /// Labeled byte region within `source_text`.
    pub span: Option<Span>,
    /// Remediation hint.
    pub help: Option<String>,
    /// A mechanical edit that resolves this diagnostic, when one is certain.
    pub fix: Option<Fix>,
}

/// A mechanical edit that resolves a diagnostic: the structured half of a
/// "did you mean" suggestion, which the prose tail renders for a reader and an
/// editor applies as a quick fix.
///
/// The span is into the diagnostic's own `source_text`, so a fix is always an
/// edit to the file the diagnostic already points into — never a second file
/// the reader is not looking at. It is independent of the diagnostic's own
/// span, which anchors where a reader should *look* and is regularly not where
/// the typed characters are: `use:` errors caret the macro's name key, several
/// lines above the misspelled target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fix {
    /// What to call the action in a UI.
    pub title: String,
    /// The byte region of the diagnostic's `source_text` to replace.
    pub span: Span,
    /// The text to put there.
    pub replacement: String,
}

impl Diag {
    /// An error diagnostic.
    pub fn error(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            severity: Severity::Error,
            message: message.into(),
            source_name: None,
            source_text: None,
            span: None,
            help: None,
            fix: None,
        }
    }

    /// A warning diagnostic.
    pub fn warning(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            ..Self::error(code, message)
        }
    }

    /// Attach the source this diagnostic points into.
    #[must_use]
    pub fn with_source(mut self, name: impl Into<String>, text: Arc<str>) -> Self {
        self.source_name = Some(name.into());
        self.source_text = Some(text);
        self
    }

    /// Attach a labeled span (clamped by the caller against the source length).
    #[must_use]
    pub fn with_span(mut self, span: Span) -> Self {
        self.span = Some(span);
        self
    }

    /// Attach a remediation hint.
    #[must_use]
    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }

    /// Attach the quick fix that rewrites `old` to `new` — but only when the
    /// edit is *certain*: `new` is a real suggestion, and `old` occurs **exactly
    /// once, as a whole token, in this diagnostic's own source text**.
    ///
    /// Each condition is a way to be wrong, so each one declines rather than
    /// approximates. The rule is deliberately about the file and not about the
    /// span: a diagnostic anchors where a *reader* should look, which is often
    /// broader or narrower than the token — `use:` errors caret the macro's name
    /// key, several lines above the misspelled target. Scoping the search to the
    /// span would have made almost every fix undiscoverable.
    ///
    /// What the file scope still guarantees is the one that matters: a fix only
    /// ever edits the file the diagnostic is already pointing into. A lowering
    /// error anchored on the feature step that invoked a macro searches the
    /// *feature*, where a pack's misspelling does not appear — so it finds
    /// nothing and offers nothing, rather than editing the healthy file. And a
    /// token appearing twice is ambiguous, so picking one would be a guess.
    ///
    /// Call it *after* `with_source` — it reads the source text.
    #[must_use]
    pub fn with_fix_replacing(mut self, old: &str, new: Option<&str>) -> Self {
        let (Some(new), Some(text)) = (new, self.source_text.as_ref()) else {
            return self;
        };
        let mut hits = text
            .match_indices(old)
            .filter(|&(at, _)| on_identifier_boundaries(text, at, old.len()));
        let (Some((start, _)), None) = (hits.next(), hits.next()) else {
            return self;
        };
        self.fix = Some(Fix {
            title: format!("replace `{old}` with `{new}`"),
            span: Span {
                start,
                end: start + old.len(),
            },
            replacement: new.to_owned(),
        });
        self
    }
}

/// Is `region[at..at + len]` a whole token — not the tail of a longer word?
///
/// Without this, replacing `search` in a span that reads `the research index`
/// would edit the middle of an unrelated word. `.` and `-` count as part of a
/// token because proef names use them (`task.search`, `content-type`).
fn on_identifier_boundaries(region: &str, at: usize, len: usize) -> bool {
    let part = |c: char| c.is_alphanumeric() || matches!(c, '_' | '-' | '.');
    let before = region[..at].chars().next_back().is_none_or(|c| !part(c));
    let after = region[at + len..].chars().next().is_none_or(|c| !part(c));
    before && after
}

/// Outcome of a front-end stage: diagnostics (user fault, exit 2 when any is an
/// error) or a non-diagnostic core failure.
#[derive(Debug, thiserror::Error)]
pub enum FrontError {
    /// Structured findings to render (at least one has [`Severity::Error`]).
    #[error("{} diagnostic(s)", .0.len())]
    Diagnostics(Vec<Diag>),
    /// An IO or internal failure outside the diagnostics model.
    #[error(transparent)]
    Core(#[from] crate::error::CoreError),
}

impl FrontError {
    /// The stable exit code for this failure (ADR-0009).
    pub fn exit_code(&self) -> crate::error::ExitCode {
        match self {
            Self::Diagnostics(_) => crate::error::ExitCode::UserError,
            Self::Core(err) => err.exit_code(),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::*;

    #[test]
    fn spans_clamp_into_the_source() {
        let span = Span::clamped(5, 12, 8);
        assert_eq!((span.start, span.end), (5, 8));
        let span = Span::clamped(10, 12, 8);
        assert!(span.is_empty());
    }

    /// The quick fix exists to be *applied*, so every way it could edit the
    /// wrong bytes has to end in no fix rather than in a plausible one.
    #[test]
    fn a_fix_is_offered_only_when_the_edit_is_certain() {
        let text: Arc<str> = Arc::from("macros:\n  wrapper:\n    steps:\n      - use: serch\n");
        let anchored = |old: &str, new: Option<&str>| {
            Diag::error("proef::test::x", "boom")
                .with_source("p.yaml", Arc::clone(&text))
                // The caret sits on the macro's name key, lines above the
                // token — the real shape of a `use:` diagnostic.
                .with_span(Span { start: 8, end: 16 })
                .with_fix_replacing(old, new)
                .fix
        };

        let fix = anchored("serch", Some("search")).expect("a lone token is replaceable");
        assert_eq!(fix.replacement, "search");
        assert_eq!(&text[fix.span.start..fix.span.end], "serch");
        assert_eq!(fix.title, "replace `serch` with `search`");

        // Nothing near enough to suggest: the message still enumerates, but
        // there is no edit to offer.
        assert!(anchored("serch", None).is_none());
        // Not in this file at all — the lowering case, where the diagnostic
        // anchors on the feature step and the typo lives in a pack. Editing
        // the healthy file would be worse than offering nothing.
        assert!(anchored("elsewhere", Some("somewhere")).is_none());
        // A source-less diagnostic has nothing to search.
        assert!(
            Diag::error("proef::test::x", "boom")
                .with_fix_replacing("serch", Some("search"))
                .fix
                .is_none()
        );
    }

    /// Two occurrences are ambiguous and a substring is not the token: both
    /// would produce an edit that compiles and is wrong.
    #[test]
    fn an_ambiguous_or_partial_match_is_not_a_fix() {
        let twice: Arc<str> = Arc::from("- use: serch\n  with: { serch: 1 }\n");
        let fix = |text: &Arc<str>, old: &str| {
            Diag::error("proef::test::x", "boom")
                .with_source("p.yaml", Arc::clone(text))
                .with_fix_replacing(old, Some("search"))
                .fix
        };
        assert!(
            fix(&twice, "serch").is_none(),
            "two occurrences: picking one would be a guess"
        );

        let inside: Arc<str> = Arc::from("  match: the research index\n");
        assert!(
            fix(&inside, "search").is_none(),
            "`search` inside `research` is not the token"
        );

        // …but the same word standing alone is.
        let alone: Arc<str> = Arc::from("  match: the search index\n");
        assert!(fix(&alone, "search").is_some());
    }

    #[test]
    fn front_error_maps_to_user_error() {
        let err = FrontError::Diagnostics(vec![Diag::error("proef::test::x", "boom")]);
        assert_eq!(err.exit_code().code(), 2);
    }
}
