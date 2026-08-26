//! Tag-expression totality: arbitrary `--tags` text must never panic and always
//! terminate under the nesting and token caps; a parsed expression evaluates
//! without panic (TESTING-STRATEGY).
//!
//! A note on this target's reach, because it did *not* find the stack overflow
//! that long `and`/`or` chains used to cause. libFuzzer's default `-max_len` is
//! 4096 bytes; at seven bytes per `@a and `, that is roughly 585 atoms, while
//! the overflow needed thousands. The target was structurally unable to reach
//! it — worth knowing before reading a green fuzz run as coverage of deep
//! input. The `MAX_TOKENS` cap that fixed it *is* reachable at this length
//! (512 tokens is about 257 atoms), so the refusal path is fuzzed even though
//! the crash never was.
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
