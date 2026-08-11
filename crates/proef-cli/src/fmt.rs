//! `proef fmt` — normalize the raw hurl blocks inside macro packs.
//!
//! v1 canonicalization (documented scope): per `hurl:`/`hurl: |` block —
//! trailing whitespace stripped, runs of blank lines collapsed to one,
//! trailing blank lines dropped. Fenced body regions (``` … ```) are the
//! bytes the test sends and stay verbatim. The YAML skeleton (comments
//! included) is never touched; blocks are located textually by indentation,
//! exactly like the pack loader's span locator.

use std::path::{Path, PathBuf};

use proef_core::error::ExitCode;

/// Format pack files under `path` (a pack file or a directory containing
/// `packs/`). `check` reports instead of rewriting (exit 1 when dirty).
pub fn fmt(path: &Path, check: bool) -> ExitCode {
    let packs = discover(path);
    if packs.is_empty() {
        crate::render::errln!("error: no pack files found under `{}`", path.display());
        return ExitCode::UserError;
    }
    let mut dirty = false;
    for pack in packs {
        let text = match std::fs::read_to_string(&pack) {
            Ok(text) => text,
            Err(err) => {
                crate::render::errln!("error: cannot read {}: {err}", pack.display());
                return ExitCode::SystemError;
            }
        };
        let formatted = normalize_pack(&text);
        if formatted != text {
            dirty = true;
            if check {
                crate::render::outln!("  needs formatting: {}", pack.display());
            } else if let Err(err) = crate::fsutil::write_atomic(&pack, &formatted) {
                crate::render::errln!("error: cannot write {}: {err}", pack.display());
                return ExitCode::SystemError;
            } else {
                crate::render::outln!("  formatted: {}", pack.display());
            }
        }
    }
    if check && dirty {
        ExitCode::TestFailure
    } else {
        if !dirty {
            crate::render::outln!("all pack blocks already canonical");
        }
        ExitCode::Success
    }
}

fn discover(path: &Path) -> Vec<PathBuf> {
    if path.is_file() {
        return vec![path.to_path_buf()];
    }
    // Same location rule as pack loading (front::pack_files) — fmt must
    // format exactly what a run would load; a plain directory of yaml files
    // (no packs/ layout) still formats as itself.
    let found = crate::front::pack_files(path).unwrap_or_default();
    if !found.is_empty() {
        return found;
    }
    let mut found: Vec<PathBuf> = std::fs::read_dir(path)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|p| p.extension().is_some_and(|e| e == "yaml" || e == "yml"))
        .collect();
    found.sort();
    found
}

/// The line ending to write back: whichever the file already uses. `fmt`
/// normalizes hurl blocks, not line endings — rewriting a CRLF checkout
/// wholesale would leave `fmt --check` permanently dirty for an author who
/// changed nothing.
fn dominant_ending(text: &str) -> &'static str {
    let crlf = text.matches("\r\n").count();
    let lf = text.matches('\n').count() - crlf;
    if crlf > lf { "\r\n" } else { "\n" }
}

/// Split `text` into `(content, terminator)` pairs, keeping each line's own
/// ending. `str::lines()` throws the terminator away, so rebuilding with a
/// single ending homogenizes a mixed-endings file — a rewrite outside this
/// command's stated scope (hurl blocks, not line endings), and one that turns
/// `fmt --check` red on a file whose blocks are already canonical.
fn split_lines(text: &str) -> Vec<(&str, &str)> {
    let mut out = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        // A final line with no terminator at all ends the walk.
        let Some(index) = rest.find('\n') else {
            out.push((rest, ""));
            break;
        };
        let (line, tail) = rest.split_at(index + 1);
        out.push(if let Some(content) = line.strip_suffix("\r\n") {
            (content, "\r\n")
        } else {
            (&line[..line.len() - 1], "\n")
        });
        rest = tail;
    }
    out
}

/// Normalize every `hurl:` block-scalar body in the pack text.
fn normalize_pack(text: &str) -> String {
    let ending = dominant_ending(text);
    // `(content, terminator)`; the terminator travels with its line so an
    // untouched line is written back byte-for-byte.
    let mut out: Vec<(String, &str)> = Vec::new();
    let mut lines = split_lines(text).into_iter().peekable();
    while let Some((line, term)) = lines.next() {
        out.push((line.trim_end().to_owned(), term));
        let trimmed = line.trim_start();
        let key = trimmed.strip_prefix("- ").unwrap_or(trimmed);
        if !(key.starts_with("hurl:") && key.trim_end().ends_with('|')) {
            continue;
        }
        // Collect the block body: lines more indented than the key.
        let key_indent = line.len() - line.trim_start().len();
        let mut body: Vec<(&str, &str)> = Vec::new();
        while let Some((next, _)) = lines.peek() {
            let is_blank = next.trim().is_empty();
            let indent = next.len() - next.trim_start().len();
            if !is_blank && indent <= key_indent {
                break;
            }
            if let Some(pair) = lines.next() {
                body.push(pair);
            }
        }
        // Canonicalize: collapse blank runs, drop trailing blanks — but a
        // fenced region (``` … ```) is the exact body the test sends, so
        // every byte inside it (blanks and trailing whitespace included)
        // stays verbatim.
        let mut canonical: Vec<(String, &str)> = Vec::new();
        let mut in_fence = false;
        for (body_line, body_term) in body {
            let is_fence_delimiter = body_line.trim_start().starts_with("```");
            if in_fence {
                in_fence = !is_fence_delimiter;
                canonical.push((body_line.to_owned(), body_term));
                continue;
            }
            if is_fence_delimiter {
                in_fence = true;
                canonical.push((body_line.trim_end().to_owned(), body_term));
            } else if body_line.trim().is_empty() {
                if canonical.last().is_some_and(|(l, _)| l.trim().is_empty()) {
                    continue;
                }
                canonical.push((String::new(), body_term));
            } else {
                canonical.push((body_line.trim_end().to_owned(), body_term));
            }
        }
        while !in_fence && canonical.last().is_some_and(|(l, _)| l.trim().is_empty()) {
            canonical.pop();
        }
        out.extend(canonical);
    }
    let mut result = String::with_capacity(text.len());
    for (index, (line, term)) in out.iter().enumerate() {
        result.push_str(line);
        // A file whose last line had no terminator still gets one — that is a
        // normalization every formatter makes, and the only one this applies
        // outside a hurl block.
        if term.is_empty() && index + 1 == out.len() {
            result.push_str(ending);
        } else {
            result.push_str(term);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_are_canonicalized_and_yaml_untouched() {
        let input = "# comment stays\nmacros:\n  m:\n    steps:\n      - hurl: |\n          GET http://x   \n\n\n          HTTP 200\n\n    match: keep\n";
        let expected = "# comment stays\nmacros:\n  m:\n    steps:\n      - hurl: |\n          GET http://x\n\n          HTTP 200\n    match: keep\n";
        assert_eq!(normalize_pack(input), expected);
        // Idempotent.
        assert_eq!(normalize_pack(expected), expected);
    }

    #[test]
    fn fenced_bodies_stay_byte_verbatim() {
        // Blank runs and trailing whitespace inside the ``` fence are the
        // bytes the test sends — fmt must not touch them.
        let input = "macros:\n  m:\n    steps:\n      - hurl: |\n          POST http://x\n          ```\n          line1  \n\n\n          line4\n          ```\n          HTTP 200\n";
        assert_eq!(normalize_pack(input), input);
    }

    #[test]
    fn normalizing_preserves_a_crlf_file_s_line_endings() {
        // `fmt` normalizes hurl blocks, not line endings: rewriting them
        // would make `fmt --check` permanently dirty on an autocrlf
        // checkout, which is a supported way to clone this repo.
        let pack = "macros:\r\n  ping:\r\n    match: I ping\r\n    steps:\r\n      - hurl: |\r\n          GET http://x/a\r\n          HTTP 200\r\n";
        let formatted = normalize_pack(pack);
        assert!(
            formatted.contains("\r\n"),
            "CRLF input must stay CRLF: {formatted:?}"
        );
        assert!(
            !formatted.replace("\r\n", "").contains('\n'),
            "no bare LF may survive in a CRLF file: {formatted:?}"
        );
    }

    #[test]
    fn normalizing_leaves_an_lf_file_on_lf() {
        let pack = "macros:\n  ping:\n    match: I ping\n    steps:\n      - hurl: |\n          GET http://x/a\n          HTTP 200\n";
        let formatted = normalize_pack(pack);
        assert!(
            !formatted.contains('\r'),
            "LF input must stay LF: {formatted:?}"
        );
    }

    /// A file mixing CRLF and LF keeps every line's own ending. `fmt`
    /// normalizes hurl blocks, not line endings — homogenizing the file is a
    /// rewrite outside that promise, and it turned `--check` red on a pack
    /// whose blocks were already canonical. The earlier fix moved from "always
    /// LF" to "the dominant ending", which still rewrote the minority lines.
    #[test]
    fn mixed_endings_survive_when_the_blocks_are_already_canonical() {
        let input = "macros:\r\n  m:\r\n    steps:\r\n      - hurl: |\n          GET http://x\n          HTTP 200\n";
        assert_eq!(
            normalize_pack(input),
            input,
            "an already-canonical pack must come back byte-for-byte"
        );
    }

    /// Rewriting a block still leaves the surrounding YAML's endings alone.
    #[test]
    fn normalizing_a_block_does_not_touch_the_yaml_s_endings() {
        let input = "macros:\r\n  m:\r\n    steps:\r\n      - hurl: |\n          GET http://x   \n\n\n          HTTP 200\n";
        let out = normalize_pack(input);
        assert!(
            out.starts_with("macros:\r\n  m:\r\n    steps:\r\n"),
            "{out:?}"
        );
        assert!(out.contains("          GET http://x\n"), "{out:?}");
        assert!(
            !out.contains("   \n"),
            "trailing whitespace must go: {out:?}"
        );
    }
}
