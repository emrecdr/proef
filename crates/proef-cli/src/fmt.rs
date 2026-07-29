//! `proef fmt` — normalize the raw hurl blocks inside macro packs.
//!
//! v1 canonicalization (documented scope): per `hurl:`/`hurl: |` block —
//! trailing whitespace stripped, runs of blank lines collapsed to one,
//! trailing blank lines dropped. The YAML skeleton (comments included) is
//! never touched; blocks are located textually by indentation, exactly like
//! the pack loader's span locator.

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
        let Ok(text) = std::fs::read_to_string(&pack) else {
            eprintln!("error: cannot read {}", pack.display());
            return ExitCode::SystemError;
        };
        let formatted = normalize_pack(&text);
        if formatted != text {
            dirty = true;
            if check {
                println!("  needs formatting: {}", pack.display());
            } else if std::fs::write(&pack, &formatted).is_err() {
                eprintln!("error: cannot write {}", pack.display());
                return ExitCode::SystemError;
            } else {
                println!("  formatted: {}", pack.display());
            }
        }
    }
    if check && dirty {
        ExitCode::TestFailure
    } else {
        if !dirty {
            println!("all pack blocks already canonical");
        }
        ExitCode::Success
    }
}

fn discover(path: &Path) -> Vec<PathBuf> {
    if path.is_file() {
        return vec![path.to_path_buf()];
    }
    let packs_dir = if path.join("packs").is_dir() {
        path.join("packs")
    } else {
        path.to_path_buf()
    };
    let mut found: Vec<PathBuf> = std::fs::read_dir(packs_dir)
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
            body.push(lines.next().unwrap_or_default().trim_end().to_owned());
        }
        // Canonicalize: collapse blank runs, drop trailing blanks.
        let mut canonical: Vec<String> = Vec::new();
        for body_line in body {
            if body_line.trim().is_empty() {
                if canonical.last().is_some_and(|l| l.trim().is_empty()) {
                    continue;
                }
                canonical.push(String::new());
            } else {
                canonical.push(body_line);
            }
        }
        while canonical.last().is_some_and(|l| l.trim().is_empty()) {
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
        let input = "# comment stays\ntemplates:\n  m:\n    steps:\n      - hurl: |\n          GET http://x   \n\n\n          HTTP 200\n\n    match: keep\n";
        let expected = "# comment stays\ntemplates:\n  m:\n    steps:\n      - hurl: |\n          GET http://x\n\n          HTTP 200\n    match: keep\n";
        assert_eq!(normalize_pack(input), expected);
        // Idempotent.
        assert_eq!(normalize_pack(expected), expected);
    }
}
