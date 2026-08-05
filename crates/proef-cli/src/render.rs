//! Render core diagnostics with miette (ADR-0009: miette lives **only** here).
//!
//! Core produces structured [`Diag`]s with byte spans; this module wraps them
//! into miette diagnostics with labeled source snippets. Color is disabled
//! under `NO_COLOR` (and the snapshot suite relies on that).

use miette::{Diagnostic, LabeledSpan, Severity};
use proef_core::diag::Diag;

/// Print a line to stdout, tolerating a closed pipe: `proef … | head` must
/// end the pipeline quietly (exit contract, never a 101 panic), so
/// `BrokenPipe` is swallowed; any other stdout failure surfaces on stderr.
macro_rules! outln {
    ($($arg:tt)*) => {{
        use ::std::io::Write as _;
        if let Err(err) = writeln!(::std::io::stdout(), $($arg)*)
            && err.kind() != ::std::io::ErrorKind::BrokenPipe
        {
            eprintln!("error: cannot write to stdout: {err}");
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
pub fn print_all(diags: &[Diag]) {
    let (errors, warnings): (Vec<_>, Vec<_>) = diags
        .iter()
        .partition(|d| d.severity == proef_core::diag::Severity::Error);
    for diag in errors.iter().chain(warnings.iter()) {
        let report = miette::Report::new(Rendered::from(*diag));
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
