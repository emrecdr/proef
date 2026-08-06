//! `proef init` — write a minimal working suite into an empty (or partly
//! populated) directory.
//!
//! The files mirror the ones `docs/GETTING-STARTED.md` teaches, so the
//! scaffold and the tutorial cannot drift into two different starting shapes.
//! Nothing is ever overwritten: an existing file is reported and left alone,
//! which makes a second run a no-op and removes any need for a `--force` flag
//! that could destroy authored work.

use std::path::{Path, PathBuf};

use proef_core::error::ExitCode;

const CONFIG: &str = r#"# proef.toml — project configuration.
# Variables live here, never in .feature files: packs read them as ${url:…} / ${vars:…}.
[run]
suite = "suite"                    # `proef test` needs no path argument

[url]
# ${url:base} resolves from here; PROEF_BASE_URL overrides it when set.
base = "${env:PROEF_BASE_URL:-http://127.0.0.1:8787}"
"#;

const FEATURE: &str = r#"Feature: Directory search
  Scenario: A known record is found
    Given the service is healthy
    When the operator searches for "Acme"
"#;

const PACK: &str = r"macros:
  health:
    match: the service is healthy
    steps:
      - hurl: |
          GET ${url:base}/health
          HTTP 200

  search:
    params: [term]
    match: the operator searches for {term}
    steps:
      - name: search records for ${term}
        hurl: |
          GET ${url:base}/search
          [Query]
          q: ${term}
          HTTP 200
";

/// Scaffold a suite under `dir`, then install the pack schema and print the
/// next command.
pub fn init(dir: &Path) -> ExitCode {
    let pack_path = dir.join("suite/packs/api.yaml");
    let files: [(PathBuf, &str); 3] = [
        (dir.join("proef.toml"), CONFIG),
        (dir.join("suite/case.feature"), FEATURE),
        (pack_path.clone(), PACK),
    ];

    let mut created = 0usize;
    let mut skipped = 0usize;
    for (path, contents) in &files {
        if path.exists() {
            crate::render::outln!("  skipped {} (already exists)", path.display());
            skipped += 1;
            continue;
        }
        let parent = crate::fsutil::parent_dir(path);
        if let Err(err) = std::fs::create_dir_all(&parent) {
            crate::render::errln!("error: cannot create {}: {err}", parent.display());
            return ExitCode::SystemError;
        }
        if let Err(err) = std::fs::write(path, contents) {
            crate::render::errln!("error: cannot write {}: {err}", path.display());
            return ExitCode::SystemError;
        }
        crate::render::outln!("  created {}", path.display());
        created += 1;
    }

    // The same install path `proef schema --add-to` runs — one implementation
    // of "write the schema and the modeline", not two.
    let schema_exit = crate::commands::schema(std::slice::from_ref(&pack_path));
    if schema_exit != ExitCode::Success {
        return schema_exit;
    }

    if created == 0 {
        crate::render::outln!("\nnothing to create — {skipped} file(s) already present");
    } else {
        crate::render::outln!("\ncreated {created} file(s), skipped {skipped}");
    }
    crate::render::outln!("next: proef test --dry-run");
    ExitCode::Success
}
