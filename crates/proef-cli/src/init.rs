//! `proef init` — write a minimal working suite into an empty (or partly
//! populated) directory.
//!
//! The files mirror the ones `docs/GETTING-STARTED.md` teaches, so the
//! scaffold and the tutorial cannot drift into two different starting shapes.
//! Nothing is ever overwritten: an existing file is reported and left alone,
//! which makes a second run a no-op and removes any need for a `--force` flag
//! that could destroy authored work.
//!
//! The scaffold is a deliberately smaller suite than the tutorial builds by
//! hand: no `Then the first hit is record "r-1"` step, no `firstHit`
//! `expect:` macro, no `${secret:apiToken}`, and `search` targets a plain
//! `/search` route instead of the tutorial's real one. Pasting the tutorial's
//! `Then` line into the scaffold's feature file fails with
//! `bind::unbound_step` — the two are not meant to be identical, just
//! consistent with each other everywhere they overlap.

use std::path::{Path, PathBuf};

use proef_core::error::ExitCode;

/// The `[url] base` value the scaffold writes, verbatim.
///
/// Exported so the run can recognise a config nobody has edited yet and say so
/// (`exec::note_untouched_scaffold`) instead of leaving a first-time reader with
/// a bare connection error. `scaffold_base_matches_the_config_template` keeps
/// this and [`CONFIG`] from drifting apart — `concat!` cannot build one from the
/// other, so a test carries the invariant.
pub(crate) const SCAFFOLD_BASE: &str = "${env:PROEF_BASE_URL:-http://127.0.0.1:8787}";

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

const PACK: &str = r"# Replace these paths with your own API's — ${url:base} is set in proef.toml.
macros:
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

// Mirrors the root `.gitignore`'s `proef runtime outputs` entry: the first
// run under the scaffold produces both, and `proef.toml` (project config) is
// the only proef-owned file meant to be committed.
const GITIGNORE: &str = ".proef-runs/\n.proef-state.json\n";

/// Scaffold a suite under `dir`, then install the pack schema and print the
/// next command.
pub fn init(dir: &Path) -> ExitCode {
    let pack_path = dir.join("suite/packs/api.yaml");
    let files: [(PathBuf, &str); 4] = [
        (dir.join("proef.toml"), CONFIG),
        (dir.join("suite/case.feature"), FEATURE),
        (pack_path.clone(), PACK),
        (dir.join(".gitignore"), GITIGNORE),
    ];

    let mut created = 0usize;
    let mut skipped = 0usize;
    let mut pack_created = false;
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
        if *path == pack_path {
            pack_created = true;
        }
    }

    // Only install the schema/modeline into the pack `init` itself just wrote:
    // running it unconditionally would rewrite a pack this same run reported
    // as "skipped (already exists)", silently breaking the never-overwrite
    // guarantee above.
    if pack_created {
        // The same install path `proef schema --add-to` runs — one implementation
        // of "write the schema and the modeline", not two — but in preserving
        // mode: `init` never overwrites, and the schema is a file a user may
        // legitimately have hand-written. It is NOT in the `files` array above,
        // so the never-overwrite loop never saw it; the guard has to be here.
        let schema_path = crate::fsutil::parent_dir(&pack_path).join(crate::commands::SCHEMA_FILE);
        let schema_existed = schema_path.exists();
        let schema_exit = crate::commands::schema(std::slice::from_ref(&pack_path), false);
        if schema_exit != ExitCode::Success {
            return schema_exit;
        }
        if schema_existed {
            skipped += 1;
        } else {
            created += 1;
        }
    } else if crate::fsutil::parent_dir(&pack_path)
        .join(crate::commands::SCHEMA_FILE)
        .exists()
    {
        // Naming the install command when the schema is already installed sends
        // the reader to do work that is already done.
        crate::render::outln!(
            "  {} already exists — editor completion is already installed beside it",
            pack_path.display()
        );
    } else {
        crate::render::outln!(
            "  {} already exists — run `proef schema --add-to {}` to install editor completion",
            pack_path.display(),
            pack_path.display()
        );
    }

    if created == 0 {
        crate::render::outln!("\nnothing to create — {skipped} file(s) already present");
    } else {
        crate::render::outln!("\ncreated {created} file(s), skipped {skipped}");
    }
    // The printed command must actually work from the cwd `init` was run in:
    // a scaffold under a subdirectory needs `cd` first, since `proef.toml`
    // discovery only walks up from the working directory, never down into a
    // path argument. The scaffold's routes are placeholders nothing serves,
    // so the next command says that too — otherwise dry-run's green light
    // walks straight into a red `proef test`.
    let next_test = if dir == Path::new(".") {
        "proef test --dry-run".to_owned()
    } else {
        format!("cd {} && proef test --dry-run", dir.display())
    };
    crate::render::outln!(
        "next: {next_test}  (then point ${{url:base}} at your API — the scaffold's routes are placeholders)"
    );
    ExitCode::Success
}

#[cfg(test)]
mod tests {
    use super::{CONFIG, SCAFFOLD_BASE};

    /// The sentinel and the template it describes are two literals; nothing in
    /// the type system keeps them equal. If the scaffold's `base` changes and
    /// this constant does not, the "still the scaffold" note silently stops
    /// firing — the failure mode is a message that never appears, which no
    /// other test would notice.
    #[test]
    fn scaffold_base_matches_the_config_template() {
        assert!(
            CONFIG.contains(&format!("base = \"{SCAFFOLD_BASE}\"")),
            "SCAFFOLD_BASE drifted from CONFIG's `[url] base` line:\n{CONFIG}"
        );
    }
}
