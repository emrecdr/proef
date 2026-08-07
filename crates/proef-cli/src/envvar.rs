//! Reading environment variables so a value proef cannot read is never
//! mistaken for one the user did not set.

/// The value of `name`, or `None` when it is genuinely unset.
///
/// `std::env::var` collapses two different situations into `Err`: the
/// variable is absent, and the variable is set to bytes that are not valid
/// UTF-8. `.ok()` erases that distinction, so a value proef cannot read
/// silently becomes "the user did not set it" — and proef then acts on the
/// wrong input and reports the wrong cause. Callers that need the raw bytes
/// (paths, presence checks) use `std::env::var_os`, which has no such
/// ambiguity and is not affected.
pub(crate) fn read(name: &str) -> Result<Option<String>, String> {
    match std::env::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => Err(format!(
            "environment variable `{name}` is set but its value is not valid UTF-8 — \
             unset it, or correct the value"
        )),
    }
}

// Edition 2024 made `std::env::set_var`/`remove_var` unsafe fns (mutating the
// environment races any other thread that reads it), so exercising this
// module's env-var behavior needs `unsafe` blocks despite the workspace-wide
// `unsafe_code = "deny"` lint. nextest runs one test per process, so there is
// no such thread to race.
#[cfg(test)]
#[allow(unsafe_code)]
mod tests {
    use super::*;

    #[test]
    fn an_unset_variable_reads_as_absent() {
        assert_eq!(read("PROEF_TEST_DEFINITELY_UNSET_XYZ"), Ok(None));
    }

    #[test]
    fn a_set_variable_reads_as_its_value() {
        let name = "PROEF_TEST_ENVVAR_PLAIN";
        // SAFETY: nextest runs one test per process, and this variable name
        // is unique to this test, so no other thread observes the mutation.
        unsafe { std::env::set_var(name, "value") };
        let got = read(name);
        unsafe { std::env::remove_var(name) };
        assert_eq!(got, Ok(Some("value".to_owned())));
    }

    #[cfg(unix)]
    #[test]
    fn a_non_utf8_value_is_an_error_not_absence() {
        use std::os::unix::ffi::OsStrExt as _;
        let name = "PROEF_TEST_ENVVAR_NON_UTF8";
        // 0xFF is not valid UTF-8 in any position.
        let bad = std::ffi::OsStr::from_bytes(&[0x66, 0xff, 0x6f]);
        // SAFETY: as above — one process per test, name unique to this test.
        unsafe { std::env::set_var(name, bad) };
        let got = read(name);
        unsafe { std::env::remove_var(name) };
        let Err(message) = got else {
            panic!("a non-UTF-8 value must not read as absent: {got:?}");
        };
        assert!(
            message.contains(name),
            "the message must name the variable so the user can find it: {message}"
        );
    }
}
