//! Tag expressions (ADR-0004 tags) — the `--tags` filter grammar.
//!
//! A boolean expression over scenario tags: `and`, `or`, `not`, and parentheses,
//! with atoms written `@smoke` or `smoke` (the `@` is optional — scenario tags
//! are stored without it). Precedence, highest to lowest: `not` → `and` → `or`.
//! This *replaces* the old comma-separated OR list; there is one selection
//! mechanism, not two.
//!
//! The parser lives in the sans-IO core so selection is deterministic and
//! fuzzable: `and`/`or` chains parse iteratively (no stack growth on long
//! chains), `not`/parenthesis nesting is depth-capped, and every malformed input
//! returns `Err` rather than panicking.

/// Deepest `not`/parenthesis nesting accepted — far beyond any real expression,
/// but a hard stop so a fuzzer cannot overflow the stack.
const MAX_DEPTH: usize = 64;

/// A parsed `--tags` expression. Build with [`parse`]; test with [`TagExpr::eval`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TagExpr {
    /// A single tag atom, normalized without its leading `@`.
    Tag(String),
    /// Logical negation.
    Not(Box<TagExpr>),
    /// Logical conjunction.
    And(Box<TagExpr>, Box<TagExpr>),
    /// Logical disjunction.
    Or(Box<TagExpr>, Box<TagExpr>),
}

impl TagExpr {
    /// Whether a scenario carrying `tags` (each without a leading `@`) is
    /// selected by this expression.
    pub fn eval(&self, tags: &[String]) -> bool {
        match self {
            TagExpr::Tag(want) => tags.iter().any(|tag| atom_matches(want, tag)),
            TagExpr::Not(inner) => !inner.eval(tags),
            TagExpr::And(left, right) => left.eval(tags) && right.eval(tags),
            TagExpr::Or(left, right) => left.eval(tags) || right.eval(tags),
        }
    }
}

/// Does one atom select one tag? An atom without `*`/`?` is literal equality
/// — the pre-glob behavior, bit-identical. With metacharacters it is an
/// anchored glob: `*` spans any run (including empty), `?` exactly one
/// character. Full-anchored on purpose (`@FRD-*` must not select `@my-FRD-x`)
/// and deliberately no character classes — `or` is the expression language's
/// alternation, and half-a-glob that silently degrades to a literal is the
/// silent-no-match class the correctness series existed to kill. Case stays
/// sensitive, matching every other comparison in this grammar (Robot
/// Framework folds case; the divergence is recorded in AUTHORING).
fn atom_matches(pattern: &str, tag: &str) -> bool {
    if !pattern.contains(['*', '?']) {
        return pattern == tag;
    }
    let pattern: Vec<char> = pattern.chars().collect();
    let tag: Vec<char> = tag.chars().collect();
    glob_at(&pattern, &tag)
}

/// Anchored glob over chars (`?` is one *character*, never one byte) with
/// backtracking on `*` only — linear for the single-`*` patterns real
/// expressions use, and input length is bounded by tag length, so the worst
/// case stays harmless.
fn glob_at(pattern: &[char], text: &[char]) -> bool {
    match pattern.split_first() {
        None => text.is_empty(),
        Some(('*', rest)) => (0..=text.len()).any(|skip| glob_at(rest, &text[skip..])),
        Some(('?', rest)) => !text.is_empty() && glob_at(rest, &text[1..]),
        Some((literal, rest)) => text.first() == Some(literal) && glob_at(rest, &text[1..]),
    }
}

/// Parse a `--tags` expression. The `Err` is a user-facing message (exit 2); an
/// empty or whitespace-only input is an error, not a match-all (absence of the
/// flag means match-all, and the CLI never calls this for that case).
pub fn parse(input: &str) -> Result<TagExpr, String> {
    let tokens = tokenize(input);
    if tokens.is_empty() {
        return Err("empty tag expression".to_owned());
    }
    let mut parser = Parser { tokens, pos: 0 };
    let expr = parser.parse_or(0)?;
    if parser.pos != parser.tokens.len() {
        return Err("unexpected trailing tokens in tag expression".to_owned());
    }
    Ok(expr)
}

/// A lexed token. `and`/`or`/`not` are keywords only as whole words; any other
/// word (e.g. `android`) is a tag atom.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Tok {
    And,
    Or,
    Not,
    LParen,
    RParen,
    Tag(String),
}

/// Split into tokens: parens are their own tokens, whitespace separates words,
/// and a word matching a keyword becomes an operator (else a `@`-stripped atom).
fn tokenize(input: &str) -> Vec<Tok> {
    fn flush(word: &mut String, tokens: &mut Vec<Tok>) {
        if word.is_empty() {
            return;
        }
        tokens.push(match word.as_str() {
            "and" => Tok::And,
            "or" => Tok::Or,
            "not" => Tok::Not,
            other => Tok::Tag(other.trim_start_matches('@').to_owned()),
        });
        word.clear();
    }

    let mut tokens = Vec::new();
    let mut word = String::new();
    for ch in input.chars() {
        match ch {
            '(' => {
                flush(&mut word, &mut tokens);
                tokens.push(Tok::LParen);
            }
            ')' => {
                flush(&mut word, &mut tokens);
                tokens.push(Tok::RParen);
            }
            ch if ch.is_whitespace() => flush(&mut word, &mut tokens),
            ch => word.push(ch),
        }
    }
    flush(&mut word, &mut tokens);
    tokens
}

/// Recursive-descent parser over the token stream.
struct Parser {
    tokens: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.tokens.get(self.pos)
    }

    fn bump(&mut self) -> Option<Tok> {
        let tok = self.tokens.get(self.pos).cloned();
        if tok.is_some() {
            self.pos += 1;
        }
        tok
    }

    /// `or_expr := and_expr ("or" and_expr)*` — lowest precedence, iterative.
    fn parse_or(&mut self, depth: usize) -> Result<TagExpr, String> {
        let mut left = self.parse_and(depth)?;
        while matches!(self.peek(), Some(Tok::Or)) {
            self.pos += 1;
            let right = self.parse_and(depth)?;
            left = TagExpr::Or(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    /// `and_expr := not_expr ("and" not_expr)*` — iterative.
    fn parse_and(&mut self, depth: usize) -> Result<TagExpr, String> {
        let mut left = self.parse_not(depth)?;
        while matches!(self.peek(), Some(Tok::And)) {
            self.pos += 1;
            let right = self.parse_not(depth)?;
            left = TagExpr::And(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    /// `not_expr := "not" not_expr | primary` — the one recursive operator.
    fn parse_not(&mut self, depth: usize) -> Result<TagExpr, String> {
        if depth > MAX_DEPTH {
            return Err("tag expression nested too deep".to_owned());
        }
        if matches!(self.peek(), Some(Tok::Not)) {
            self.pos += 1;
            Ok(TagExpr::Not(Box::new(self.parse_not(depth + 1)?)))
        } else {
            self.parse_primary(depth)
        }
    }

    /// `primary := "(" or_expr ")" | tag`.
    fn parse_primary(&mut self, depth: usize) -> Result<TagExpr, String> {
        if depth > MAX_DEPTH {
            return Err("tag expression nested too deep".to_owned());
        }
        match self.bump() {
            Some(Tok::LParen) => {
                let inner = self.parse_or(depth + 1)?;
                match self.bump() {
                    Some(Tok::RParen) => Ok(inner),
                    _ => Err("unclosed `(` in tag expression".to_owned()),
                }
            }
            Some(Tok::Tag(tag)) if !tag.is_empty() => Ok(TagExpr::Tag(tag)),
            Some(Tok::Tag(_)) => Err("empty tag atom (a bare `@`)".to_owned()),
            Some(Tok::And | Tok::Or) => {
                Err("expected a tag or `(`, found a binary operator".to_owned())
            }
            Some(Tok::Not) => Err("misplaced `not` in tag expression".to_owned()),
            Some(Tok::RParen) => Err("unexpected `)` in tag expression".to_owned()),
            None => Err("unexpected end of tag expression".to_owned()),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn tags(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn bare_atom_matches_with_or_without_at() {
        let expr = parse("@api").unwrap();
        assert_eq!(expr, parse("api").unwrap());
        assert!(expr.eval(&tags(&["api", "slow"])));
        assert!(!expr.eval(&tags(&["web"])));
    }

    #[test]
    fn precedence_is_not_over_and_over_or() {
        // `api or web and not slow` == `api or (web and (not slow))`.
        let expr = parse("api or web and not slow").unwrap();
        assert!(expr.eval(&tags(&["api", "slow"]))); // api wins the or
        assert!(expr.eval(&tags(&["web"]))); // web and not slow
        assert!(!expr.eval(&tags(&["web", "slow"]))); // web but slow
        assert!(!expr.eval(&tags(&["desktop"])));
    }

    #[test]
    fn parentheses_override_precedence() {
        let expr = parse("(api or web) and not slow").unwrap();
        assert!(expr.eval(&tags(&["web"])));
        assert!(!expr.eval(&tags(&["web", "slow"])));
        assert!(!expr.eval(&tags(&["api", "slow"])));
    }

    #[test]
    fn not_binds_tighter_than_and() {
        let expr = parse("not api and web").unwrap();
        assert!(expr.eval(&tags(&["web"])));
        assert!(!expr.eval(&tags(&["api", "web"])));
    }

    #[test]
    fn a_word_containing_a_keyword_is_still_an_atom() {
        let expr = parse("android").unwrap();
        assert!(expr.eval(&tags(&["android"])));
    }

    #[test]
    fn malformed_expressions_are_errors_not_panics() {
        for bad in [
            "", "   ", "and", "api and", "or web", "(api", "api)", "not", "@", "()",
            "api web", // two atoms, no operator
        ] {
            assert!(parse(bad).is_err(), "expected error for {bad:?}");
        }
    }

    #[test]
    fn deep_nesting_is_rejected_not_overflowed() {
        let deep = "not ".repeat(MAX_DEPTH + 5) + "api";
        assert!(parse(&deep).is_err());
        let parens = "(".repeat(MAX_DEPTH + 5);
        assert!(parse(&parens).is_err());
    }

    #[test]
    fn long_or_chain_is_iterative() {
        // Thousands of `or`s must parse without recursing (no stack overflow).
        let atoms: Vec<String> = (0..5000).map(|i| format!("t{i}")).collect();
        let expr = parse(&atoms.join(" or ")).unwrap();
        assert!(expr.eval(&tags(&["t4999"])));
        assert!(!expr.eval(&tags(&["missing"])));
    }

    /// Glob atoms: anchored, `*` spans, `?` is one char, and a metachar-free
    /// atom is exact equality (bit-identical pre-glob behavior).
    #[test]
    fn glob_atoms_match_anchored() {
        let tags = |list: &[&str]| list.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        let sel = |expr: &str, list: &[&str]| parse(expr).unwrap().eval(&tags(list));
        assert!(sel("@FRD-*", &["FRD-3.1"]));
        assert!(sel("@sync-*", &["sync-note"]));
        assert!(
            !sel("@FRD-*", &["my-FRD-3.1"]),
            "anchored: no substring match"
        );
        assert!(
            !sel("@FRD-*", &["FRD"]),
            "the literal prefix is required in full"
        );
        assert!(sel("@FRD-*", &["FRD-"]), "star spans the empty run");
        assert!(!sel("@FRD-*", &["frd-3.1"]), "case stays sensitive");
        assert!(sel("@case-?", &["case-7"]));
        assert!(!sel("@case-?", &["case-42"]), "? is exactly one char");
        assert!(sel("@case-?", &["case-é"]), "? is one CHAR, not one byte");
        assert!(sel("not @wip-*", &["api"]));
        assert!(!sel("not @wip-*", &["wip-auth"]));
        assert!(
            sel("@*", &["anything"]),
            "bare * means: has at least one tag"
        );
        assert!(!sel("@*", &[]), "bare * never selects the untagged");
    }

    proptest::proptest! {
        /// A pattern with no metacharacters selects exactly what equality
        /// selects — the glob extension cannot change any pre-glob result.
        #[test]
        fn a_metachar_free_atom_is_equality(
            atom in "[a-zA-Z0-9_.-]{1,20}",
            tag in "[a-zA-Z0-9_.-]{1,20}",
        ) {
            let expr = parse(&format!("@{atom}")).unwrap();
            proptest::prop_assert_eq!(expr.eval(std::slice::from_ref(&tag)), atom == tag);
        }
    }
}
