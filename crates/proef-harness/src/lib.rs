//! The proef nextest/IDE harness (ADR-0008, US-12).
//!
//! The `scenarios` test target (`harness = false`, libtest-mimic) exposes one
//! `Trial` per proef scenario, so `cargo nextest run -p proef-harness` and IDE
//! test UIs drive suites with zero custom protocol work.
//!
//! Configuration (environment):
//! - `PROEF_HARNESS_SUITE` — the feature file/dir to expose (unset = no
//!   trials; keeps plain workspace test runs green without a fixture).
//! - `PROEF_BIN` — path to the `proef` binary (default: `proef` on PATH).
//! - plus whatever the suite itself needs (`PROEF_BASE_URL`,
//!   `PROEF_SECRET_*`, …).
//!
//! *Unset* and *set to something unreadable* are different answers. Unset
//! means "expose nothing", on purpose. A variable set to bytes that are not
//! valid UTF-8 means the caller asked for something and the harness cannot
//! tell what — so it exposes one failing `proef::config` trial instead,
//! rather than silently running a different binary or reporting green having
//! listed no tests at all.
//!
//! **Parallelism caveat**: each Trial is its own `proef` process, and the
//! persistent World (`.proef-state.json`) has no cross-process lock — a suite
//! using `saveAs: global` across scenarios is last-writer-wins under parallel
//! trials. Run such suites with `--test-threads=1` (or nextest
//! `test-threads = 1`); per-scenario process isolation is the design,
//! cross-trial ordering is deliberately undefined.

/// The value of `name`, or `None` when it is genuinely unset.
///
/// `std::env::var` reports "absent" and "set to bytes that are not valid
/// UTF-8" as the same `Err`. Collapsing them here would be worse than
/// elsewhere: an unreadable `PROEF_BIN` silently runs a *different* binary,
/// and an unreadable `PROEF_HARNESS_SUITE` exposes no trials at all while
/// `cargo test` reports green — the "never run zero tests green" outcome the
/// harness already guards against for contract drift.
pub fn env_var(name: &str) -> Result<Option<String>, String> {
    match std::env::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => Err(format!(
            "environment variable `{name}` is set but its value is not valid UTF-8 — \
             unset it, or correct the value"
        )),
    }
}

#[cfg(test)]
#[allow(unsafe_code)]
mod tests {
    use super::*;

    #[test]
    fn an_unset_variable_reads_as_absent() {
        assert_eq!(env_var("PROEF_HARNESS_TEST_DEFINITELY_UNSET"), Ok(None));
    }

    #[test]
    fn a_set_variable_reads_as_its_value() {
        let name = "PROEF_HARNESS_TEST_PLAIN";
        // SAFETY: nextest runs one test per process and this name is unique to
        // this test, so no other thread observes the mutation.
        unsafe { std::env::set_var(name, "value") };
        let got = env_var(name);
        unsafe { std::env::remove_var(name) };
        assert_eq!(got, Ok(Some("value".to_owned())));
    }

    #[cfg(unix)]
    #[test]
    fn a_non_utf8_value_is_an_error_not_absence() {
        use std::os::unix::ffi::OsStrExt as _;
        let name = "PROEF_HARNESS_TEST_NON_UTF8";
        let bad = std::ffi::OsStr::from_bytes(&[0x66, 0xff, 0x6f]);
        // SAFETY: as above — one process per test, name unique to this test.
        unsafe { std::env::set_var(name, bad) };
        let got = env_var(name);
        unsafe { std::env::remove_var(name) };
        let Err(message) = got else {
            panic!("a non-UTF-8 value must not read as absent: {got:?}");
        };
        assert!(message.contains(name), "must name the variable: {message}");
    }
}
