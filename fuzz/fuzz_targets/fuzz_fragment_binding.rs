//! The rules that compare a pack against a fragment corpus (ADR-0018, R9-2):
//! `ref:` resolution, `bind:` keys nothing reads, a `bind:` colliding with a
//! variable the fragment supplies itself, and a placeholder neither bound nor
//! supplied.
//!
//! **Structure-aware on purpose.** `fuzz_pack_load` feeds raw bytes at the YAML
//! parser, which is the right shape for parser totality and the wrong one for
//! these rules: reaching them means discovering a *valid* pack — `macros:`, a
//! macro, `steps:`, a `ref:` — and a corpus declaring the matching name, all at
//! once. Measured, a byte-oriented target did not manage it in 1.45 million
//! runs; every input died in the parser and the fragment logic was covered on
//! paper only. So this target **builds** a well-formed pack and corpus and
//! spends the whole budget on the name space where the rules actually live:
//! collisions, empty names, near-misses, and clashes between the three scopes.
//!
//! The corpus is read by a **synthetic scanner**, not hurl's. `proef-core` is
//! engine-agnostic by construction (ADR-0002) — its fragment logic is defined
//! against `ScannedFile`, so any scanner exercises it. That keeps this
//! workspace free of native libraries: cargo dependencies are package-level, so
//! one engine-dependent target would compile hurl for every target here. Hurl's
//! own scanner is covered by proptest in `proef-engine-hurl`, where those
//! libraries already are.
#![no_main]

use std::sync::Arc;

use libfuzzer_sys::fuzz_target;
use proef_core::engine::{
    FragmentScanError, FragmentSupport, OPTION_FAMILIES, ScannedFile, ScannedFragment,
    StepKindSpec,
};
use proef_core::pack::{self, FragmentCorpus, PackSource};

/// Names the generated YAML can carry without needing quoting, so a pack built
/// from fuzzer bytes always parses. Includes `.` because fragment names are
/// free-form dotted (`admin.search`), and the empty string is reachable too —
/// an empty name is exactly the kind of edge these rules must survive.
const ALPHABET: &[u8] = b"abcdeAB01._-";

/// A short YAML-safe identifier drawn from `bytes`.
fn ident(bytes: &[u8]) -> String {
    bytes
        .iter()
        .take(12)
        .map(|b| ALPHABET[*b as usize % ALPHABET.len()] as char)
        .collect()
}

/// Zero or one element: the empty string means "this fragment has none".
fn one(value: &str) -> Vec<String> {
    if value.is_empty() {
        Vec::new()
    } else {
        vec![value.to_owned()]
    }
}

/// The corpus half's grammar: one fragment per generated file, described
/// entirely by the fields the core reasons about.
fn synthetic_scan(text: &str) -> Result<ScannedFile, FragmentScanError> {
    let mut file = ScannedFile::default();
    for (index, line) in text.lines().enumerate() {
        // `name|placeholder|supplied|option-index`
        let mut parts = line.split('|');
        let Some(name) = parts.next() else { continue };
        let placeholder = parts.next().unwrap_or_default();
        let supplied = parts.next().unwrap_or_default();
        let option = parts.next().unwrap_or_default();
        file.fragments.push(ScannedFragment {
            name: name.to_owned(),
            text: line.to_owned(),
            line: index + 1,
            placeholders: one(placeholder),
            // Mapped onto the real vocabulary rather than passed through: a
            // family only the engine knows *silences* the clash check by
            // design, so free-form strings would spend the budget proving the
            // check does not run.
            declared_options: option
                .bytes()
                .next()
                .map(|b| OPTION_FAMILIES[b as usize % OPTION_FAMILIES.len()].to_owned())
                .into_iter()
                .collect(),
            supplied_variables: one(supplied),
        });
    }
    Ok(file)
}

const KINDS: &[StepKindSpec] = &[StepKindSpec {
    prefix: "hurl",
    schema: "true",
    validate: None,
    fragments: Some(FragmentSupport {
        ext: "frag",
        scan: synthetic_scan,
    }),
    options: None,
}];

fuzz_target!(|data: &[u8]| {
    // Seven independent knobs, each a slice of the input. Fixed-width so a
    // mutation to one does not reshape the others — the fuzzer can then walk a
    // single name toward a collision instead of rebuilding the whole document.
    if data.len() < 7 {
        return;
    }
    let field = |n: usize| ident(&data[n..(n + 3).min(data.len())]);

    let declared = field(0); // the name the corpus declares
    let referenced = field(1); // the name the pack asks for — equal often enough to matter
    let placeholder = field(2); // what the fragment reads
    let supplied = field(3); // what the fragment supplies to itself
    let option = field(4); // an option family it declares
    let bind_key = field(5); // what the pack binds
    let bind_value = field(6);

    let corpus_line = format!("{declared}|{placeholder}|{supplied}|{option}");
    let corpus = FragmentCorpus::new(
        vec![PackSource {
            // The name must carry the claimed extension or the scan skips the
            // file — `FragmentSupport::claims` is the one place that question is
            // answered, and this goes through it like every other reader.
            name: "fuzz.frag".to_owned(),
            text: Arc::from(corpus_line.as_str()),
        }],
        KINDS,
    );

    // A pack that always parses, so every input reaches the rules under test.
    // `bind:` appears at both pack and step scope: most-specific-wins is itself
    // one of the rules, and a single scope could never exercise it.
    let pack_text = format!(
        "bind:\n  {bind_key}: \"{bind_value}\"\nmacros:\n  m:\n    match: the step runs\n    \
         bind: {{ {bind_key}: \"{bind_value}\" }}\n    steps:\n      - ref: {referenced}\n"
    );
    let sources = [PackSource {
        name: "fuzz.yaml".to_owned(),
        text: Arc::from(pack_text.as_str()),
    }];
    let _ = pack::load(&sources, &corpus, KINDS);
});
