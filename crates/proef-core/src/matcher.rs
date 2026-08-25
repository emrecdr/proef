//! The `{name}` step matcher: cucumber-expression-style patterns binding Gherkin
//! prose to macros (TECH-SPEC §4.3).
//!
//! A pattern is literal text with `{name}` captures (`I search for {term}`).
//! Matching is **anchored and leftmost**: literals must appear in order (the
//! whole text must be consumed), and each capture extends to the leftmost
//! occurrence of the next literal. Captured values are trimmed; a value wrapped
//! in symmetric double or single quotes sheds them (quotes preserve inner
//! spaces and commas exactly).
//!
//! Guard rails ([`pattern_problems`], run at pack load — validation pass 1):
//! a pattern must contain literal text to anchor on, adjacent captures are
//! rejected (the single-pass matcher cannot split them), braces must be
//! balanced, and every capture must name a declared param.

use std::collections::{BTreeMap, BTreeSet};

/// One token of a `match:` pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Token {
    /// Literal text that must appear verbatim.
    Literal(String),
    /// A `{name}` capture.
    Capture(String),
}

/// Split a pattern into [`Token`]s. An unclosed `{` degrades the remainder into
/// a literal — [`pattern_problems`] rejects it at load; the matcher stays total.
pub fn tokenize(pattern: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut rest = pattern;
    while let Some(open) = rest.find('{') {
        if open > 0 {
            tokens.push(Token::Literal(rest[..open].to_owned()));
        }
        let after = &rest[open + 1..];
        if let Some(close) = after.find('}') {
            tokens.push(Token::Capture(after[..close].trim().to_owned()));
            rest = &after[close + 1..];
        } else {
            tokens.push(Token::Literal(rest.to_owned()));
            return tokens;
        }
    }
    if !rest.is_empty() {
        tokens.push(Token::Literal(rest.to_owned()));
    }
    tokens
}

/// Match `text` against `pattern`, returning the captured args, or `None` when
/// the pattern does not apply. Total: never panics, any inputs.
pub fn match_pattern(pattern: &str, text: &str) -> Option<BTreeMap<String, String>> {
    let tokens = tokenize(pattern);
    let mut args = BTreeMap::new();
    let mut rest = text;
    let mut index = 0;
    while index < tokens.len() {
        match &tokens[index] {
            Token::Literal(lit) => rest = rest.strip_prefix(lit.as_str())?,
            Token::Capture(name) => {
                let next_literal = match tokens.get(index + 1) {
                    Some(Token::Literal(lit)) if !lit.is_empty() => Some(lit.as_str()),
                    _ => None,
                };
                let value = match next_literal {
                    Some(lit) => {
                        let end = rest.find(lit)?;
                        let (value, remainder) = rest.split_at(end);
                        rest = remainder;
                        value
                    }
                    None => std::mem::take(&mut rest),
                };
                args.insert(name.clone(), shed_quotes(value.trim()).to_owned());
            }
        }
        index += 1;
    }
    rest.is_empty().then_some(args)
}

/// Strip one symmetric pair of surrounding quotes (`"…"` or `'…'`), keeping the
/// inner text exactly — the quoting mechanism that preserves spaces and commas.
fn shed_quotes(value: &str) -> &str {
    for quote in ['"', '\''] {
        if value.len() >= 2
            && let Some(inner) = value
                .strip_prefix(quote)
                .and_then(|v| v.strip_suffix(quote))
        {
            return inner;
        }
    }
    value
}

/// One problem found in a `match:` pattern (validation pass 1), typed so each
/// maps to a stable diagnostic code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatternProblem {
    /// No literal text to anchor on — a bare capture matches every step.
    NoAnchor,
    /// Two captures with nothing between them — the matcher cannot split them.
    AdjacentCaptures {
        /// First capture name.
        first: String,
        /// Second capture name.
        second: String,
    },
    /// A stray `{`/`}` inside literal text (unclosed or unescaped).
    UnsupportedBraces {
        /// The literal fragment near the problem.
        near: String,
    },
    /// An empty `{}` capture.
    EmptyCapture,
    /// A capture that names no declared param.
    UnknownCapture {
        /// The capture name as written.
        name: String,
        /// Closest declared param, when one is near.
        suggestion: Option<String>,
    },
    /// The same capture written twice — a later match would silently
    /// overwrite the earlier binding.
    DuplicateCapture {
        /// The repeated capture name.
        name: String,
    },
}

impl PatternProblem {
    /// The stable diagnostic code for this problem.
    pub fn code(&self) -> &'static str {
        match self {
            Self::NoAnchor => "proef::pack::pattern_no_anchor",
            Self::AdjacentCaptures { .. } => "proef::pack::adjacent_captures",
            Self::UnsupportedBraces { .. } => "proef::pack::pattern_braces",
            Self::EmptyCapture => "proef::pack::pattern_empty_capture",
            Self::UnknownCapture { .. } => "proef::pack::pattern_unknown_capture",
            Self::DuplicateCapture { .. } => "proef::pack::pattern_duplicate_capture",
        }
    }
}

impl std::fmt::Display for PatternProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoAnchor => f.write_str(
                "pattern has no literal text to match on — a bare capture matches every step",
            ),
            Self::AdjacentCaptures { first, second } => write!(
                f,
                "adjacent captures `{{{first}}}{{{second}}}` are ambiguous — put literal text between them"
            ),
            Self::UnsupportedBraces { near } => write!(
                f,
                "unsupported `{{` or `}}` in pattern (near `{near}`) — captures are written `{{name}}`"
            ),
            Self::EmptyCapture => f.write_str("empty capture `{}`"),
            Self::UnknownCapture { name, suggestion } => {
                let hint = suggestion
                    .as_ref()
                    .map(|p| format!(" (did you mean `{p}`?)"))
                    .unwrap_or_default();
                write!(f, "capture `{{{name}}}` is not a declared param{hint}")
            }
            Self::DuplicateCapture { name } => write!(
                f,
                "capture `{{{name}}}` appears more than once — a later match would silently overwrite the earlier value"
            ),
        }
    }
}

/// The problems found in a `match:` pattern (validation pass 1); empty = sound.
pub fn pattern_problems(pattern: &str, params: &[String]) -> Vec<PatternProblem> {
    let tokens = tokenize(pattern);
    let mut problems = Vec::new();

    let has_anchor = tokens
        .iter()
        .any(|t| matches!(t, Token::Literal(lit) if !lit.trim().is_empty()));
    if !has_anchor {
        problems.push(PatternProblem::NoAnchor);
    }

    for pair in tokens.windows(2) {
        if let [Token::Capture(a), Token::Capture(b)] = pair {
            problems.push(PatternProblem::AdjacentCaptures {
                first: a.clone(),
                second: b.clone(),
            });
        }
    }

    let mut seen_names = BTreeSet::new();
    for token in &tokens {
        let Token::Capture(name) = token else {
            continue;
        };
        if !seen_names.insert(name.as_str()) {
            let dup = PatternProblem::DuplicateCapture { name: name.clone() };
            if !problems.contains(&dup) {
                problems.push(dup);
            }
        }
    }

    for token in &tokens {
        match token {
            Token::Literal(lit) if lit.contains('{') || lit.contains('}') => {
                problems.push(PatternProblem::UnsupportedBraces {
                    near: lit.trim().to_owned(),
                });
            }
            Token::Capture(name) if name.is_empty() => {
                problems.push(PatternProblem::EmptyCapture);
            }
            Token::Capture(name) if !params.iter().any(|p| p == name) => {
                problems.push(PatternProblem::UnknownCapture {
                    name: name.clone(),
                    suggestion: closest(name, params.iter().map(String::as_str))
                        .map(ToOwned::to_owned),
                });
            }
            _ => {}
        }
    }
    problems
}

/// The literal skeleton of a pattern (captures dropped) — the comparison basis
/// for closest-pattern suggestions on unbound steps.
pub fn literal_skeleton(pattern: &str) -> String {
    tokenize(pattern)
        .into_iter()
        .filter_map(|t| match t {
            Token::Literal(s) => Some(s),
            Token::Capture(_) => None,
        })
        .collect()
}

/// Rank `pattern` against the partially-typed prose `typed`, for completion
/// ordering. Lower sorts first: the returned tuple is `(tier, tiebreak)`.
///
/// Comparison is against the pattern's [`literal_skeleton`] (captures dropped —
/// the prose the author actually types), case-insensitively, in tiers:
/// tier 0 the skeleton starts with `typed`; tier 1 `typed` occurs inside the
/// skeleton (tiebreak = the match position); tier 2 the edit distance between
/// `typed` and the skeleton's leading `typed`-length slice (the prefix-aligned
/// distance, not the whole-pattern distance). An empty `typed` is a prefix of
/// every skeleton, so all patterns share `(0, 0)` and keep their prior order.
///
/// This is a distinct problem from [`closest`], which finds the single most
/// likely mistyped *complete* step; the two coexist.
pub fn prefix_rank(typed: &str, pattern: &str) -> (u8, usize) {
    let skeleton = literal_skeleton(pattern).to_lowercase();
    let typed = typed.to_lowercase();
    if skeleton.starts_with(&typed) {
        (0, 0)
    } else if let Some(idx) = skeleton.find(&typed) {
        (1, idx)
    } else {
        // Prefix-aligned distance: compare `typed` against only the leading
        // `typed`-length slice of the skeleton, so divergence past the typed
        // portion does not inflate the score the way whole-pattern distance does.
        let n = typed.chars().count();
        let prefix: String = skeleton.chars().take(n).collect();
        (2, levenshtein(&typed, &prefix))
    }
}

/// Group pattern macros that differ **only** in their captures — i.e. share a
/// [`literal_skeleton`]. Returns, for each such macro, the sorted names of its
/// near-duplicate siblings. Pure and deterministic (sorted throughout); the
/// caller (`proef macros`) surfaces it as an authoring advisory, never a gate.
///
/// Skeleton-equality is the deliberately tight signal: two patterns whose fixed
/// text is identical and that differ only where a `{capture}` sits are genuinely
/// confusable, whereas patterns with distinct literals (`shows the note` vs
/// `shows the attachment`) keep distinct skeletons and are left alone — so a
/// legitimately similar family is not flagged.
pub fn near_duplicate_macros<'a>(
    macros: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> BTreeMap<String, Vec<String>> {
    let mut by_skeleton: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    for (name, pattern) in macros {
        by_skeleton
            .entry(literal_skeleton(pattern))
            .or_default()
            .push(name);
    }
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for names in by_skeleton.values_mut() {
        if names.len() < 2 {
            continue;
        }
        names.sort_unstable();
        for &name in names.iter() {
            let siblings = names
                .iter()
                .filter(|&&other| other != name)
                .map(|&other| other.to_owned())
                .collect();
            out.insert(name.to_owned(), siblings);
        }
    }
    out
}

/// The candidate closest to `input` by edit distance, within the shared
/// "did you mean" threshold. `None` when nothing is close.
pub fn closest<'a>(input: &str, candidates: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    candidates
        .map(|c| (levenshtein(input, c), c))
        .filter(|(distance, _)| *distance <= SUGGESTION_DISTANCE)
        .min_by_key(|(distance, _)| *distance)
        .map(|(_, c)| c)
}

/// Maximum edit distance for a "did you mean" suggestion.
const SUGGESTION_DISTANCE: usize = 3;

/// The tail every "unknown name" diagnostic shares: the nearest spelling
/// when one is near, else the valid set itself — small sets verbatim, large
/// ones counted, with `listing` naming the command that prints them in full.
///
/// Every call site used to end `closest(…).map(…).unwrap_or_default()`:
/// below the threshold the tail went silent, and for `unknown_step_kind`
/// that meant a valid set of size **one** stayed unnamed while the message
/// said "not claimed by any registered engine". An empty candidate set
/// yields an empty tail — there is nothing to say.
pub fn suggest_or_enumerate<'a>(
    input: &str,
    candidates: impl Iterator<Item = &'a str> + Clone,
    listing: Option<&str>,
) -> String {
    if let Some(close) = closest(input, candidates.clone()) {
        return format!(" — did you mean `{close}`?");
    }
    let mut known: Vec<&str> = candidates.collect();
    known.sort_unstable();
    known.dedup();
    match known.len() {
        0 => String::new(),
        1..=6 => format!(" (known: {})", known.join(", ")),
        n => match listing {
            Some(listing) => format!(" ({n} known — {listing})"),
            None => format!(" ({n} known)"),
        },
    }
}

/// Levenshtein edit distance over chars (small inputs; O(a·b) rolling row).
pub fn levenshtein(a: &str, b: &str) -> usize {
    let b_chars: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b_chars.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut previous_diagonal = row[0];
        row[0] = i + 1;
        for (j, cb) in b_chars.iter().enumerate() {
            let substitution = previous_diagonal + usize::from(ca != *cb);
            previous_diagonal = row[j + 1];
            row[j + 1] = substitution.min(row[j] + 1).min(previous_diagonal + 1);
        }
    }
    row[b_chars.len()]
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn params(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn literal_pattern_matches_exactly() {
        assert_eq!(
            match_pattern(
                "the activity channel is activated and ready",
                "the activity channel is activated and ready"
            ),
            Some(BTreeMap::new())
        );
        assert_eq!(
            match_pattern("I create a record", "I create a records"),
            None
        );
        assert_eq!(
            match_pattern("I create a record", "so I create a record"),
            None
        );
    }

    #[test]
    fn captures_split_on_leftmost_literal() {
        let args = match_pattern(
            "the record {name} is resolved",
            "the record W-${run:id} is resolved",
        )
        .unwrap();
        assert_eq!(args["name"], "W-${run:id}");
    }

    #[test]
    fn multi_capture_binds_in_order() {
        let args =
            match_pattern("I search {index} for {term}", "I search records for Jansen").unwrap();
        assert_eq!(args["index"], "records");
        assert_eq!(args["term"], "Jansen");
    }

    #[test]
    fn quoted_capture_preserves_inner_text() {
        let args = match_pattern("I search for {term}", r#"I search for "Jansen, A. ""#).unwrap();
        assert_eq!(args["term"], "Jansen, A. ");
        let args = match_pattern("I search for {term}", "I search for 'de Vries'").unwrap();
        assert_eq!(args["term"], "de Vries");
    }

    #[test]
    fn unquoted_capture_is_trimmed() {
        let args = match_pattern("I search for {term} now", "I search for   Jansen   now").unwrap();
        assert_eq!(args["term"], "Jansen");
    }

    #[test]
    fn trailing_capture_takes_the_rest() {
        let args = match_pattern("say {message}", "say hello world").unwrap();
        assert_eq!(args["message"], "hello world");
    }

    #[test]
    fn guard_rails_reject_bad_patterns() {
        assert!(
            !pattern_problems("{a}", &params(&["a"])).is_empty(),
            "no anchor"
        );
        assert!(
            !pattern_problems("do {a}{b} now", &params(&["a", "b"])).is_empty(),
            "adjacent captures"
        );
        assert!(
            !pattern_problems("do {a", &params(&["a"])).is_empty(),
            "unclosed brace"
        );
        assert!(
            !pattern_problems("do {} now", &[]).is_empty(),
            "empty capture"
        );
        assert!(
            pattern_problems("do {a} now", &params(&["a"])).is_empty(),
            "sound pattern"
        );
    }

    #[test]
    fn unknown_capture_gets_a_suggestion() {
        let problems = pattern_problems("I log in as {rol}", &params(&["role"]));
        assert_eq!(problems.len(), 1);
        assert_eq!(problems[0].code(), "proef::pack::pattern_unknown_capture");
        assert!(
            problems[0].to_string().contains("did you mean `role`?"),
            "{}",
            problems[0]
        );
    }

    #[test]
    fn closest_respects_the_threshold() {
        assert_eq!(
            closest("serch", ["search", "create"].into_iter()),
            Some("search")
        );
        assert_eq!(closest("zzzzzz", ["search", "create"].into_iter()), None);
    }

    #[test]
    fn near_duplicate_macros_flags_capture_only_differences() {
        let dups = near_duplicate_macros([
            ("loginRole", "the user {role} logs in"),
            ("loginName", "the user {name} logs in"),
            ("showNote", "the board shows the note"),
            ("showItem", "the board shows the scheduled item"),
        ]);
        // Same skeleton "the user  logs in" → mutual near-duplicates.
        assert_eq!(dups.get("loginRole"), Some(&vec!["loginName".to_owned()]));
        assert_eq!(dups.get("loginName"), Some(&vec!["loginRole".to_owned()]));
        // Distinct literals (`note` vs `scheduled item`) → not flagged.
        assert!(
            !dups.contains_key("showNote"),
            "distinct literals stay unflagged"
        );
        assert!(!dups.contains_key("showItem"));
    }

    #[test]
    fn duplicate_captures_are_rejected_once_per_name() {
        let params = vec!["x".to_owned()];
        let problems = pattern_problems("move {x} to {x} and {x}", &params);
        let dups: Vec<_> = problems
            .iter()
            .filter(|p| matches!(p, PatternProblem::DuplicateCapture { name } if name == "x"))
            .collect();
        assert_eq!(dups.len(), 1, "{problems:?}");
    }

    #[test]
    fn prefix_rank_tiers_prefix_over_substring_over_miss() {
        // "I gr" is a prefix of "I greet {who}" (skeleton "I greet ") -> tier 0.
        let greet = prefix_rank("I gr", "I greet {who}");
        // "gr" appears inside "I grab {thing}" as a substring but not a prefix -> tier 1.
        let grab = prefix_rank("gr", "I grab {thing}");
        // "I gr" is neither prefix nor substring of "the note is saved" -> tier 2.
        let note = prefix_rank("I gr", "the note is saved");
        assert_eq!(greet.0, 0);
        assert_eq!(grab.0, 1);
        assert_eq!(note.0, 2);
        // Ordering: prefix < substring < miss.
        assert!(greet < grab);
        assert!(grab < note);
    }

    #[test]
    fn prefix_rank_prefix_match_beats_large_full_pattern_distance() {
        // The bug fix: "I gr" is a full-pattern edit-distance of ~9 from
        // "I greet {who}" (so `closest` would reject it), but prefix_rank ranks it
        // top (tier 0) and well above an unrelated pattern.
        let greet = prefix_rank("I gr", "I greet {who}");
        let unrelated = prefix_rank("I gr", "the note is saved");
        assert!(greet < unrelated);
        // Sanity: closest, the old substrate, finds nothing at this distance.
        assert!(closest("I gr", ["I greet {who}"].into_iter()).is_none());
    }

    #[test]
    fn prefix_rank_tier2_uses_prefix_aligned_distance_not_full_pattern() {
        // "I greex" is neither a prefix nor a substring of "I greet {who}" -> tier 2.
        // Its distance is measured against the LEADING 7 chars of the skeleton
        // ("i greet"), giving 1 — far smaller than against an unrelated pattern.
        let near = prefix_rank("I greex", "I greet {who}");
        let far = prefix_rank("I greex", "the note is saved");
        assert_eq!(near.0, 2);
        assert_eq!(far.0, 2);
        assert_eq!(near.1, 1);
        assert!(
            near.1 < far.1,
            "prefix-aligned distance ranks the near pattern first"
        );
    }

    #[test]
    fn prefix_rank_is_case_insensitive() {
        assert_eq!(prefix_rank("i gr", "I greet {who}").0, 0);
        assert_eq!(prefix_rank("I GR", "i greet {who}").0, 0);
    }

    #[test]
    fn prefix_rank_empty_typed_is_uniform_tier0() {
        // Empty prefix is a prefix of everything -> all (0, 0) -> stable order.
        assert_eq!(prefix_rank("", "I greet {who}"), (0, 0));
        assert_eq!(prefix_rank("", "the note is saved"), (0, 0));
    }

    mod properties {
        #![allow(clippy::ignored_unit_patterns)]

        use super::*;
        use proptest::prelude::*;

        proptest! {
            /// Total on arbitrary inputs: never panics (fuzz target mirrors this).
            #[test]
            fn matcher_never_panics(pattern in ".{0,60}", text in ".{0,120}") {
                let _ = match_pattern(&pattern, &text);
                let _ = pattern_problems(&pattern, &[]);
            }

            /// A sound single-capture pattern round-trips a quoted value exactly.
            #[test]
            fn quote_round_trip(value in "[^\"{}]{0,40}") {
                let text = format!("I search for \"{value}\" now");
                let args = match_pattern("I search for {term} now", &text).unwrap();
                prop_assert_eq!(args["term"].as_str(), value.as_str());
            }

            /// Adjacent captures are always rejected by the guard rails.
            #[test]
            fn adjacent_captures_always_rejected(a in "[a-z]{1,8}", b in "[a-z]{1,8}") {
                let pattern = format!("go {{{a}}}{{{b}}} end");
                let names = vec![a.clone(), b.clone()];
                prop_assert!(!pattern_problems(&pattern, &names).is_empty());
            }

            /// Unquoted round-trip: generated capture text without quote/brace
            /// noise survives bind → args intact (modulo the documented trim).
            #[test]
            fn unquoted_round_trip(value in "[a-zA-Z0-9_-]{1,30}") {
                let text = format!("the record {value} is resolved");
                let args = match_pattern("the record {name} is resolved", &text).unwrap();
                prop_assert_eq!(args["name"].as_str(), value.as_str());
            }
        }
    }
}
