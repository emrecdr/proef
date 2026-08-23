//! Render core diagnostics with miette (ADR-0009: miette lives **only** here).
//!
//! Core produces structured [`Diag`]s with byte spans; this module wraps them
//! into miette diagnostics with labeled source snippets. Color is disabled
//! under `NO_COLOR` (and the snapshot suite relies on that).

use miette::{Diagnostic, LabeledSpan, Severity};
use proef_core::diag::Diag;

/// Set when a write to stdout failed for any reason other than a closed
/// pipe. Read once, at `main`'s exit funnel: output proef could not deliver
/// must not look like success. A closed pipe is not a failure — `proef … |
/// head` ends the pipeline on purpose.
static STDOUT_FAILED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub(crate) fn note_stdout_failure() {
    STDOUT_FAILED.store(true, std::sync::atomic::Ordering::Relaxed);
}

pub(crate) fn stdout_failed() -> bool {
    STDOUT_FAILED.load(std::sync::atomic::Ordering::Relaxed)
}

/// Print a line to stdout, tolerating a closed pipe: `proef … | head` must
/// end the pipeline quietly (exit contract, never a 101 panic), so
/// `BrokenPipe` is swallowed; any other stdout failure surfaces on stderr
/// and latches [`STDOUT_FAILED`] so the exit funnel can turn it into a
/// system error instead of whatever the command's own verdict was.
macro_rules! outln {
    ($($arg:tt)*) => {{
        use ::std::io::Write as _;
        if let Err(err) = writeln!(::std::io::stdout(), $($arg)*)
            && err.kind() != ::std::io::ErrorKind::BrokenPipe
        {
            crate::render::note_stdout_failure();
            crate::render::errln!("error: cannot write to stdout: {err}");
        }
    }};
}
pub(crate) use outln;

/// Print a line to stderr, tolerating a closed pipe. Diagnostics go to stderr,
/// so `proef … |& head` must end the pipeline quietly (exit contract, never a
/// 101 panic). `BrokenPipe` is swallowed; any other stderr error is also
/// dropped — stderr is the only diagnostic channel, so a broken stderr has
/// nowhere left to report to (writing the failure to stdout would corrupt
/// program output).
macro_rules! errln {
    ($($arg:tt)*) => {{
        use ::std::io::Write as _;
        let _ = writeln!(::std::io::stderr(), $($arg)*);
    }};
}
pub(crate) use errln;

/// Install the global miette hook: unicode graphics, color only when the
/// terminal wants it.
pub fn install() {
    let _ = miette::set_hook(Box::new(|_| {
        use std::io::IsTerminal as _;
        let theme = if std::env::var_os("NO_COLOR").is_some() || !std::io::stderr().is_terminal() {
            miette::GraphicalTheme::unicode_nocolor()
        } else {
            miette::GraphicalTheme::unicode()
        };
        Box::new(miette::GraphicalReportHandler::new_themed(theme))
    }));
}

/// Print diagnostics to stderr, errors first.
///
/// Identical `(code, message)` warnings collapse to their first occurrence
/// with a repeat count — one authored mistake in a macro shared by fifty
/// scenarios is one warning on a console, not fifty. The collapse lives HERE,
/// at rendering, and not in the front end's aggregation, because
/// `front.warnings` also feeds SARIF, where one result *per site* is the
/// point: a code-scanning consumer wants every anchor, a human wants the
/// class. Deduping the shared list threw away the anchors SARIF exists to
/// carry (R17 deep-audit).
pub fn print_all(diags: &[Diag]) {
    let (errors, warnings): (Vec<_>, Vec<_>) = diags
        .iter()
        .partition(|d| d.severity == proef_core::diag::Severity::Error);
    let mut kept: Vec<&Diag> = Vec::new();
    let mut repeats: Vec<usize> = Vec::new();
    let mut index_of: std::collections::BTreeMap<(&str, &str), usize> =
        std::collections::BTreeMap::new();
    for warning in &warnings {
        let key = (warning.code, warning.message.as_str());
        if let Some(&at) = index_of.get(&key) {
            repeats[at] += 1;
        } else {
            index_of.insert(key, kept.len());
            repeats.push(1);
            kept.push(warning);
        }
    }
    for diag in &errors {
        let report = miette::Report::new(Rendered::from(*diag));
        errln!("{report:?}");
    }
    for (warning, count) in kept.iter().zip(&repeats) {
        let mut shown: Diag = (**warning).clone();
        if *count > 1 {
            shown.message = format!("{} ({count} sites across the suite)", shown.message);
        }
        let report = miette::Report::new(Rendered::from(&shown));
        errln!("{report:?}");
    }
}

/// A core [`Diag`] wrapped for miette.
#[derive(Debug)]
struct Rendered {
    code: &'static str,
    message: String,
    help: Option<String>,
    severity: Severity,
    source: Option<miette::NamedSource<String>>,
    span: Option<miette::SourceSpan>,
}

impl From<&Diag> for Rendered {
    fn from(diag: &Diag) -> Self {
        let source = match (&diag.source_name, &diag.source_text) {
            (Some(name), Some(text)) => {
                Some(miette::NamedSource::new(name.clone(), text.to_string()))
            }
            _ => None,
        };
        Self {
            code: diag.code,
            message: diag.message.clone(),
            help: diag.help.clone(),
            severity: match diag.severity {
                proef_core::diag::Severity::Error => Severity::Error,
                proef_core::diag::Severity::Warning => Severity::Warning,
            },
            source,
            span: diag
                .span
                .map(|span| miette::SourceSpan::new(span.start.into(), span.len())),
        }
    }
}

impl std::fmt::Display for Rendered {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Rendered {}

impl Diagnostic for Rendered {
    fn code<'a>(&'a self) -> Option<Box<dyn std::fmt::Display + 'a>> {
        Some(Box::new(self.code))
    }

    fn severity(&self) -> Option<Severity> {
        Some(self.severity)
    }

    fn help<'a>(&'a self) -> Option<Box<dyn std::fmt::Display + 'a>> {
        self.help
            .as_ref()
            .map(|h| Box::new(h.clone()) as Box<dyn std::fmt::Display>)
    }

    fn source_code(&self) -> Option<&dyn miette::SourceCode> {
        self.source.as_ref().map(|s| s as &dyn miette::SourceCode)
    }

    fn labels(&self) -> Option<Box<dyn Iterator<Item = LabeledSpan> + '_>> {
        self.span.map(|span| {
            Box::new(std::iter::once(LabeledSpan::new_with_span(None, span)))
                as Box<dyn Iterator<Item = LabeledSpan>>
        })
    }
}

/// A step's fragment as a trailing ` (via file.hurl#name)`, empty when the step
/// ran an inline `hurl:` block (ADR-0018).
///
/// Lives here, not beside one sink, because every place that renders a failing
/// step answers the same question — *which file did this request come from* —
/// and a helper scoped to a delivery channel is how `proef test` ends up
/// printing no provenance on stderr while `report.junit.xml` from that same run
/// prints it. Takes the name rather than a step so the `Event`-driven readers
/// (`explain`) and the `StepOutcome`-driven ones (console, TAP, `JUnit`, the job
/// summary) share one spelling.
pub fn via(fragment: Option<&str>) -> String {
    fragment
        .map(|name| format!(" (via {name})"))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    // The latch is process-global and one-way; `main` reads it once at the
    // exit funnel. This pins the mechanism on every platform, including the
    // ones where a real write failure cannot be forced (macOS has no
    // `/dev/full`). It must be the only test in this binary that reads the
    // latch: nextest runs each test in its own process, so cross-test
    // pollution of `STDOUT_FAILED` is not observable — but a second reader
    // here, in the same process, would be.
    #[test]
    fn a_recorded_stdout_failure_is_visible_to_the_exit_funnel() {
        assert!(!stdout_failed(), "the latch starts clear");
        note_stdout_failure();
        assert!(stdout_failed(), "a recorded failure must be visible");
    }
}
