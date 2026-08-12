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

mod fragment;
mod session;

use proef_core::engine::{
    DoctorCheck, DoctorResult, EngineFactory, EngineSession, FragmentSupport, PayloadProbeError,
    RawOption, RawOptionValue, ScenarioCtx, StepKindSpec,
};
use proef_core::error::EngineError;

/// The exact embedded hurl release (kept in lockstep with the Cargo pin —
/// asserted by `embedded_version_matches_the_cargo_pin`).
pub const EMBEDDED_HURL_VERSION: &str = "8.0.1";

/// A parseable single-entry probe file used by doctor checks and smoke tests.
const PROBE_HURL: &str = "GET http://localhost/health\nHTTP 200\n";

/// Step kinds claimed by this engine: the `hurl:` raw block (ADR-0004,
/// TECH-SPEC §6 — the pack key doubles as the routing kind). The validate hook
/// is pack validation pass 7's probe parser; the fragment hooks are ADR-0018's
/// `.hurl` scanner, which reads named entries out of real hurl files.
const STEP_KINDS: &[StepKindSpec] = &[StepKindSpec {
    prefix: "hurl",
    schema: r#"{ "type": "string", "description": "Raw hurl entries; ${…} lowered at author time, {{…}} resolved by hurl at run time" }"#,
    validate: Some(validate_payload),
    fragments: Some(FragmentSupport {
        ext: "hurl",
        scan: fragment::scan,
    }),
    options: Some(recognise_option),
}];

/// hurl's `[Options]` keys, as the core's budget rules see them (ADR-0007).
///
/// The one place these spellings live. `retry-interval` folds into the `retry`
/// family — they are one policy, and a step's `retry:` sets both — which is the
/// same mapping [`fragment::scan`] makes from `OptionKind::RetryInterval`, now
/// written once instead of once per body form.
fn recognise_option(key: &str) -> Option<RawOption> {
    let (family, value) = match key {
        "retry" => (Some("retry"), Some(RawOptionValue::Count)),
        // No YAML twin, so it cannot be declared twice — but an infinite
        // `repeat` is exactly as unbounded as an infinite `retry`.
        "repeat" => (None, Some(RawOptionValue::Count)),
        "delay" => (Some("delay"), Some(RawOptionValue::Duration)),
        // Part of the retry policy for double-declaration purposes; its own
        // value carries no separate cap.
        "retry-interval" => (Some("retry"), None),
        _ => return None,
    };
    Some(RawOption { family, value })
}

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
        // A parseable payload with zero entries (only comments or blank
        // lines) would execute nothing while the step reports green — reject
        // it at load so dry-run and execution agree.
        Ok(file) if file.entries.is_empty() => Err(PayloadProbeError {
            line: 1,
            column: 1,
            message: "contains no hurl entries (only comments or blank lines)".to_owned(),
        }),
        Ok(_) => Ok(()),
        Err(err) => Err(PayloadProbeError {
            line: err.pos.line,
            column: err.pos.column,
            message: format!("{:?}", err.kind),
        }),
    }
}

/// Report the libcurl this binary is linked against (mirrors `curl --version`) —
/// the library `run_entries` drives during execution.
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

    /// The doc on [`EMBEDDED_HURL_VERSION`] promises lockstep with the Cargo
    /// pin — this is that assertion: a runbook pin bump that forgets the
    /// const (or vice versa) fails here, not at doctor time mid-upgrade.
    #[test]
    fn embedded_version_matches_the_cargo_pin() {
        let manifest = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.toml"),
        )
        .unwrap();
        for dep in ["hurl", "hurl_core"] {
            let needle = format!("{dep} = \"={EMBEDDED_HURL_VERSION}\"");
            assert!(
                manifest.contains(&needle),
                "workspace Cargo.toml must contain `{needle}` (ADR-0003 lockstep)"
            );
        }
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
