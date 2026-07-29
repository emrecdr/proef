//! Matcher totality: arbitrary pattern × text must never panic
//! (TESTING-STRATEGY — parser-adjacent hand-written code).
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        // Split on a char boundary — a raw byte-midpoint split panics on
        // multi-byte codepoints (found by this very target's first run).
        let mid = (0..=text.len() / 2)
            .rev()
            .find(|i| text.is_char_boundary(*i))
            .unwrap_or(0);
        let (pattern, step) = text.split_at(mid);
        let _ = proef_core::matcher::match_pattern(pattern, step);
        let _ = proef_core::matcher::pattern_problems(pattern, &["a".to_owned()]);
        let _ = proef_core::matcher::literal_skeleton(pattern);
    }
});
