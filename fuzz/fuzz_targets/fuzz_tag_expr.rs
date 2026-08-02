//! Tag-expression totality: arbitrary `--tags` text must never panic and always
//! terminate under the nesting cap; a parsed expression evaluates without panic
//! (TESTING-STRATEGY).
#![no_main]

use libfuzzer_sys::fuzz_target;
use proef_core::tags;

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        if let Ok(expr) = tags::parse(text) {
            // A parsed expression must evaluate against arbitrary tag sets.
            let _ = expr.eval(&["api".to_owned(), "slow".to_owned()]);
            let _ = expr.eval(&[]);
        }
    }
});
