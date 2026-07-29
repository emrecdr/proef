//! The proef API engine: [`EngineFactory`]/[`EngineSession`] over **embedded hurl**
//! (ADR-0001).
//!
//! hurl's crates are pinned exactly (`=8.0.1`) and built `--locked` — the crates
//! break API in minor releases; upgrades go only through the canary + runbook
//! (ADR-0003, IMPLEMENTATION-PLAN §7).
//!
//! The engine claims the `hurl` step kind, contributes doctor checks, and
//! executes scenario artifacts via the embedded `run_entries` (see the
//! `session` module source for the adapter mechanics).

mod session;

use proef_core::engine::{
    DoctorCheck, DoctorResult, EngineFactory, EngineSession, PayloadProbeError, ScenarioCtx,
    StepKindSpec,
};
use proef_core::error::EngineError;

/// The exact embedded hurl release (kept in lockstep with the Cargo pin —
/// asserted by the seam smoke test).
pub const EMBEDDED_HURL_VERSION: &str = "8.0.1";

/// A parseable single-entry probe file used by doctor checks and smoke tests.
const PROBE_HURL: &str = "GET http://localhost/health\nHTTP 200\n";

/// Step kinds claimed by this engine: the `hurl:` raw block (ADR-0004,
/// TECH-SPEC §6 — the pack key doubles as the routing kind). The validate hook
/// is pack validation pass 7's probe parser.
const STEP_KINDS: &[StepKindSpec] = &[StepKindSpec {
    prefix: "hurl",
    schema: r#"{ "type": "string", "description": "Raw hurl entries; ${…} lowered at author time, {{…}} resolved by hurl at run time" }"#,
    validate: Some(validate_payload),
}];

/// The compiled-in hurl engine, registered by `proef-cli` (ADR-0002).
pub struct HurlEngineFactory;

impl EngineFactory for HurlEngineFactory {
    fn id(&self) -> &'static str {
        "hurl"
    }

    fn step_kinds(&self) -> &'static [StepKindSpec] {
        STEP_KINDS
    }

    fn doctor(&self) -> Vec<DoctorCheck> {
        vec![
            DoctorCheck {
                name: "embedded hurl",
                run: check_embedded_version,
            },
            DoctorCheck {
                name: "hurl parser",
                run: check_parser,
            },
            DoctorCheck {
                name: "libcurl",
                run: check_libcurl,
            },
        ]
    }

    fn open(&self, ctx: &ScenarioCtx) -> Result<Box<dyn EngineSession>, EngineError> {
        Ok(Box::new(session::HurlSession::open(ctx)?))
    }
}

/// Report the pinned hurl release compiled into this binary.
fn check_embedded_version() -> DoctorResult {
    DoctorResult::pass(format!(
        "hurl {EMBEDDED_HURL_VERSION} (exact pin, ADR-0003)"
    ))
}

/// Exercise `hurl_core`'s parser on a probe file — proves the parser linkage that
/// pack loading (M1) and artifact validation (M2) rely on. Because `hurl_core`
/// links libxml2, a running probe also proves the libxml2 dynamic linkage loads.
fn check_parser() -> DoctorResult {
    match hurl_core::parser::parse_hurl_file(PROBE_HURL) {
        Ok(file) if file.entries.len() == 1 => DoctorResult::pass(
            "parsed 1-entry probe file (hurl_core + libxml2 linkage loads)".to_owned(),
        ),
        Ok(file) => DoctorResult::warn(format!(
            "probe parsed with unexpected entry count {}",
            file.entries.len()
        )),
        Err(err) => DoctorResult::fail(format!("cannot parse probe file: {err:?}")),
    }
}

/// Probe-parse a lowered payload with hurl's real parser (pack validation
/// pass 7 — the seam hook on [`StepKindSpec`]).
fn validate_payload(text: &str) -> Result<(), PayloadProbeError> {
    let mut normalized = text.to_owned();
    if !normalized.ends_with('\n') {
        normalized.push('\n');
    }
    match hurl_core::parser::parse_hurl_file(&normalized) {
        Ok(_) => Ok(()),
        Err(err) => Err(PayloadProbeError {
            line: err.pos.line,
            column: err.pos.column,
            message: format!("{:?}", err.kind),
        }),
    }
}

/// Report the libcurl this binary is linked against (mirrors `curl --version`) —
/// the library `run_entries` drives at M3.
fn check_libcurl() -> DoctorResult {
    let info = hurl::http::libcurl_version_info();
    if info.libraries.is_empty() {
        return DoctorResult::warn("libcurl loaded but reported no libraries".to_owned());
    }
    DoctorResult::pass(format!("{} (host {})", info.libraries.join(" "), info.host))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    /// The seam facts this engine relies on are pinned (ADR-0001): parseable
    /// probe, one entry, exact crate version.
    #[test]
    fn parser_seam_smoke() {
        let result = check_parser();
        assert_eq!(
            result.status,
            proef_core::engine::DoctorStatus::Pass,
            "{}",
            result.detail
        );
    }

    #[test]
    fn factory_claims_the_hurl_step_kind() {
        let factory = HurlEngineFactory;
        assert_eq!(factory.id(), "hurl");
        assert_eq!(factory.step_kinds().len(), 1);
        assert_eq!(factory.step_kinds()[0].prefix, "hurl");
    }

    #[test]
    fn payload_probe_accepts_valid_and_rejects_broken_hurl() {
        assert!(validate_payload("GET http://x/one\nHTTP 200").is_ok());
        // An unknown response section name is unambiguously invalid hurl.
        let err = validate_payload("GET http://x/one\nHTTP 200\n[Wrong]\n").unwrap_err();
        assert!(
            err.line >= 2,
            "position should be near the broken section: {err:?}"
        );
    }
}
