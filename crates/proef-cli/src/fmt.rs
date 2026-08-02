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
        eprintln!("error: no pack files found under `{}`", path.display());
        return ExitCode::UserError;
    }
    let mut dirty = false;
    for pack in packs {
        let text = match std::fs::read_to_string(&pack) {
            Ok(text) => text,
            Err(err) => {
                eprintln!("error: cannot read {}: {err}", pack.display());
                return ExitCode::SystemError;
            }
        };
        let formatted = normalize_pack(&text);
        if formatted != text {
            dirty = true;
            if check {
                crate::render::outln!("  needs formatting: {}", pack.display());
            } else if let Err(err) = crate::fsutil::write_atomic(&pack, &formatted) {
                eprintln!("error: cannot write {}: {err}", pack.display());
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

/// Normalize every `hurl:` block-scalar body in the pack text.
fn normalize_pack(text: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        out.push(line.trim_end().to_owned());
        let trimmed = line.trim_start();
        let key = trimmed.strip_prefix("- ").unwrap_or(trimmed);
        if !(key.starts_with("hurl:") && key.trim_end().ends_with('|')) {
            continue;
        }
        // Collect the block body: lines more indented than the key.
        let key_indent = line.len() - line.trim_start().len();
        let mut body: Vec<String> = Vec::new();
        while let Some(next) = lines.peek() {
            let is_blank = next.trim().is_empty();
            let indent = next.len() - next.trim_start().len();
            if !is_blank && indent <= key_indent {
                break;
            }
            body.push(lines.next().unwrap_or_default().to_owned());
        }
        // Canonicalize: collapse blank runs, drop trailing blanks — but a
        // fenced region (``` … ```) is the exact body the test sends, so
        // every byte inside it (blanks and trailing whitespace included)
        // stays verbatim.
        let mut canonical: Vec<String> = Vec::new();
        let mut in_fence = false;
        for body_line in body {
            let is_fence_delimiter = body_line.trim_start().starts_with("```");
            if in_fence {
                in_fence = !is_fence_delimiter;
                canonical.push(body_line);
                continue;
            }
            if is_fence_delimiter {
                in_fence = true;
                canonical.push(body_line.trim_end().to_owned());
            } else if body_line.trim().is_empty() {
                if canonical.last().is_some_and(|l| l.trim().is_empty()) {
                    continue;
                }
                canonical.push(String::new());
            } else {
                canonical.push(body_line.trim_end().to_owned());
            }
        }
        while !in_fence && canonical.last().is_some_and(|l| l.trim().is_empty()) {
            canonical.pop();
        }
        out.extend(canonical);
    }
    let mut result = out.join("\n");
    result.push('\n');
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
}
