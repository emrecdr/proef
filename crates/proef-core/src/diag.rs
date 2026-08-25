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
    use super::*;

    #[test]
    fn spans_clamp_into_the_source() {
        let span = Span::clamped(5, 12, 8);
        assert_eq!((span.start, span.end), (5, 8));
        let span = Span::clamped(10, 12, 8);
        assert!(span.is_empty());
    }

    #[test]
    fn front_error_maps_to_user_error() {
        let err = FrontError::Diagnostics(vec![Diag::error("proef::test::x", "boom")]);
        assert_eq!(err.exit_code().code(), 2);
    }
}
