//! Pack loader totality: arbitrary YAML bytes must error, never panic
//! (TESTING-STRATEGY).
#![no_main]

use std::sync::Arc;

use libfuzzer_sys::fuzz_target;
use proef_core::engine::StepKindSpec;
use proef_core::pack::{self, PackSource};

const KINDS: &[StepKindSpec] = &[StepKindSpec {
    prefix: "hurl",
    schema: "true",
    validate: None,
    fragments: None,
}];

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        let sources = [PackSource {
            name: "fuzz.yaml".to_owned(),
            text: Arc::from(text),
        }];
        let _ = pack::load(&sources, &pack::FragmentCorpus::empty(), KINDS);
    }
});
