//! Named hurl fragments end to end (ADR-0018).
//!
//! The premise of the feature is that **one file serves both tools**: a pack
//! `ref:`s an entry in a real `.hurl` file, and that same file — byte for byte,
//! annotation and all — still runs under stock `hurl`. Everything else about
//! fragments is an optimisation of authoring; this is the claim that justifies
//! them, so it is executed here rather than argued in a doc.
//!
//! Deliberately *not* part of the reference corpus. `tests/features` is run from
//! temp directories with settings passed by environment variable and no
//! `proef.toml` in scope, so it is config-independent by design; a scenario
//! needing `[run] fragments` would break that for every test that uses it.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};

use assert_cmd::Command;
use proef_fixture::{API_TOKEN, Fixture};

/// A corpus file as a backend team would keep it: ordinary hurl, with one added
/// comment naming the entry proef may use. Nothing else about it is proef's.
const CORPUS: &str = "\
# Admin endpoints. Owned by the backend team; proef only annotates.
# @proef admin.search
GET {{base}}/api/v1/admin/search/{{index}}
Authorization: Bearer {{apiToken}}
[Query]
q: {{q}}
HTTP 200
[Captures]
recordId: jsonpath \"$[0].id\"
";

const PACK: &str = r#"bind:
  base: ${url:base}
  apiToken: ${secret:apiToken}
macros:
  search:
    params: [q, index]
    defaults: { index: records }
    match: "the operator searches for {q}"
    bind: { q: "${q}", index: "${index}" }
    steps:
      - ref: admin.search
"#;

const FEATURE: &str = "Feature: F\n  Scenario: S\n    When the operator searches for \"Acme\"\n";

/// Write a project whose pack refs a fragment. `hurl` holds the corpus file,
/// `pack` the vocabulary — both overridable so the error cases can vary one.
fn project(hurl: &str, pack: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("tests/features/packs")).unwrap();
    std::fs::create_dir_all(root.join("tests/hurl")).unwrap();
    std::fs::write(
        root.join("proef.toml"),
        "[run]\nsuite = \"tests/features\"\nfragments = \"tests/hurl\"\n\
         [url]\nbase = \"${env:PROEF_BASE_URL}\"\n",
    )
    .unwrap();
    std::fs::write(root.join("tests/hurl/admin.hurl"), hurl).unwrap();
    std::fs::write(root.join("tests/features/packs/api.yaml"), pack).unwrap();
    std::fs::write(root.join("tests/features/a.feature"), FEATURE).unwrap();
    dir
}

fn proef_in(dir: &Path, fixture: &Fixture) -> Command {
    let mut cmd = Command::cargo_bin("proef").unwrap();
    cmd.current_dir(dir)
        .env("NO_COLOR", "1")
        .env("PROEF_BASE_URL", &fixture.base_url)
        .env("PROEF_SECRET_APITOKEN", API_TOKEN);
    cmd
}

/// The stock `hurl` binary, when this machine has one. The engine is embedded,
/// so a `hurl` on PATH is not a build requirement — the half of the proof that
/// needs it is skipped rather than faked, and says so.
fn stock_hurl() -> Option<PathBuf> {
    let probe = std::process::Command::new("hurl").arg("--version").output();
    matches!(probe, Ok(out) if out.status.success()).then(|| PathBuf::from("hurl"))
}

/// **The claim the feature exists for.** One file, two runners, both green —
/// and the file the backend team owns is never rewritten to achieve it.
#[test]
fn one_fragment_file_runs_under_proef_and_under_stock_hurl() {
    let fixture = Fixture::start().unwrap();
    let dir = project(CORPUS, PACK);
    let corpus_file = dir.path().join("tests/hurl/admin.hurl");

    // 1. proef executes it through the pack that names it.
    proef_in(dir.path(), &fixture)
        .args(["test", "--output", "json"])
        .assert()
        .code(0);

    // The corpus file is still exactly what was written — proef reads, never writes.
    assert_eq!(
        std::fs::read_to_string(&corpus_file).unwrap(),
        CORPUS,
        "a corpus proef does not own must come back byte-identical"
    );

    // 2. Stock hurl executes the same file, unmodified, with the variables the
    //    pack would have bound.
    let Some(hurl) = stock_hurl() else {
        eprintln!("note: no `hurl` on PATH — the stock-runner half of this proof was skipped");
        return;
    };
    let vars = dir.path().join("vars.txt");
    std::fs::write(
        &vars,
        format!("base={}\nindex=records\nq=Acme\n", fixture.base_url),
    )
    .unwrap();
    let out = std::process::Command::new(hurl)
        .arg("--test")
        .arg("--no-color")
        .arg("--variables-file")
        .arg(&vars)
        .arg("--secret")
        .arg(format!("apiToken={API_TOKEN}"))
        .arg(&corpus_file)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stock hurl must run the annotated file as-is:\n{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A bound secret reaches the request but never the artifact — the binding
/// renames it, which is what lets a foreign corpus keep its own variable name.
#[test]
fn a_bound_secret_never_reaches_an_artifact() {
    let fixture = Fixture::start().unwrap();
    // The corpus calls it `authToken`; the vault calls it `apiToken`.
    let hurl = CORPUS.replace("{{apiToken}}", "{{authToken}}");
    let pack = PACK.replace(
        "apiToken: ${secret:apiToken}",
        "authToken: ${secret:apiToken}",
    );
    let dir = project(&hurl, &pack);

    proef_in(dir.path(), &fixture)
        .args(["test"])
        .assert()
        .code(0);

    proef_in(dir.path(), &fixture)
        .args(["artifacts", "tests/features", "-o", "out", "--run-id", "r"])
        .assert()
        .code(0);
    for entry in std::fs::read_dir(dir.path().join("out")).unwrap().flatten() {
        let text = std::fs::read_to_string(entry.path()).unwrap_or_default();
        assert!(
            !text.contains(API_TOKEN),
            "{} leaked the secret value",
            entry.path().display()
        );
    }
}

/// The emitted artifact **is** the executed input (ADR-0010), so the shape of an
/// injected `[Options] variable:` block is a compatibility surface. Pinned here
/// rather than in the reference corpus, which carries no fragment — leaving it
/// unpinned meant the emitter's whole binding output could change unnoticed.
///
/// Deterministic without a fixture: `artifacts` emits without running, so the
/// base URL is a fixed literal and every path in the output is suite-relative.
#[test]
fn a_fragment_artifact_is_snapshot_locked() {
    let dir = project(CORPUS, PACK);
    std::fs::write(
        dir.path().join("proef.toml"),
        "[run]\nsuite = \"tests/features\"\nfragments = \"tests/hurl\"\n\
         [url]\nbase = \"http://example.test\"\n",
    )
    .unwrap();

    Command::cargo_bin("proef")
        .unwrap()
        .current_dir(dir.path())
        .env("NO_COLOR", "1")
        .args([
            "artifacts",
            "tests/features",
            "-o",
            "out",
            "--run-id",
            "snap",
        ])
        .assert()
        .code(0);

    let artifact = std::fs::read_to_string(dir.path().join("out/a--s.hurl")).unwrap();
    insta::assert_snapshot!("fragment_artifact", artifact);
}

/// `--dry-run` is the validation gate, so the fragment-side refusals have to
/// reach it. Each names the fragment file, not just the pack.
fn dry_run_error(hurl: &str, pack: &str) -> String {
    let dir = project(hurl, pack);
    let mut cmd = Command::cargo_bin("proef").unwrap();
    let assert = cmd
        .current_dir(dir.path())
        .env("NO_COLOR", "1")
        .env("PROEF_BASE_URL", "http://127.0.0.1:1")
        .args(["test", "--dry-run"])
        .assert()
        .code(2);
    String::from_utf8_lossy(&assert.get_output().stderr).into_owned()
}

#[test]
fn a_duplicate_fragment_name_is_refused() {
    let hurl = format!("{CORPUS}\n# @proef admin.search\nGET {{{{base}}}}/other\nHTTP 200\n");
    let stderr = dry_run_error(&hurl, PACK);
    assert!(
        stderr.contains("proef::pack::duplicate_fragment"),
        "{stderr}"
    );
}

#[test]
fn an_unreadable_fragment_file_is_refused() {
    let stderr = dry_run_error(
        "# @proef admin.search\nGET {{base}}/x\nHTTP notastatus\n",
        PACK,
    );
    assert!(stderr.contains("proef::pack::bad_annotation"), "{stderr}");
}

#[test]
fn an_annotation_carrying_settings_is_refused() {
    let hurl = CORPUS.replace("# @proef admin.search", "# @proef admin.search retry=3");
    let stderr = dry_run_error(&hurl, PACK);
    assert!(stderr.contains("proef::pack::bad_annotation"), "{stderr}");
    assert!(
        stderr.contains("a name and nothing else"),
        "the refusal must say why: {stderr}"
    );
}

#[test]
fn a_variable_nothing_supplies_is_refused() {
    // The corpus reads `{{index}}`; drop the binding that fed it.
    let pack = PACK.replace(
        r#"bind: { q: "${q}", index: "${index}" }"#,
        r#"bind: { q: "${q}" }"#,
    );
    let stderr = dry_run_error(CORPUS, &pack);
    assert!(
        stderr.contains("proef::lower::unbound_placeholder"),
        "{stderr}"
    );
    assert!(
        stderr.contains("index"),
        "it must name the variable: {stderr}"
    );
}

#[test]
fn a_secret_mixed_into_a_larger_binding_is_refused() {
    let pack = PACK.replace(
        "apiToken: ${secret:apiToken}",
        r#"apiToken: "Bearer ${secret:apiToken}""#,
    );
    let stderr = dry_run_error(CORPUS, &pack);
    assert!(
        stderr.contains("proef::lower::secret_in_composite_bind"),
        "{stderr}"
    );
}

/// An entry nobody annotated is inert: pointing proef at a corpus costs nothing
/// until a pack names one of its requests. Without this, adopting a suite would
/// mean auditing every entry in it first.
#[test]
fn unannotated_entries_are_inert() {
    let fixture = Fixture::start().unwrap();
    let hurl = format!("{CORPUS}\nDELETE {{{{base}}}}/api/v1/admin/records/1\nHTTP 204\n");
    let dir = project(&hurl, PACK);

    proef_in(dir.path(), &fixture)
        .args(["artifacts", "tests/features", "-o", "out", "--run-id", "r"])
        .assert()
        .code(0);
    let artifact = std::fs::read_dir(dir.path().join("out"))
        .unwrap()
        .flatten()
        .map(|e| std::fs::read_to_string(e.path()).unwrap_or_default())
        .collect::<String>();
    assert!(
        !artifact.contains("DELETE"),
        "an unannotated entry must never be executed: {artifact}"
    );
}

// ---------------------------------------------------------------------------
// Provenance: what a run record says about where a request came from
// ---------------------------------------------------------------------------

/// The same corpus, asserting a status the fixture will not return, so the
/// `ref:` step fails and a post-mortem has something to explain.
const FAILING_CORPUS: &str = "\
# Admin endpoints. Owned by the backend team; proef only annotates.
# @proef admin.search
GET {{base}}/api/v1/admin/search/{{index}}
Authorization: Bearer {{apiToken}}
[Query]
q: {{q}}
HTTP 418
";

/// Both body forms in one pack: a `ref:` step and an inline `hurl:` block, so
/// one run shows what provenance each carries.
const MIXED_PACK: &str = r#"bind:
  base: ${url:base}
  apiToken: ${secret:apiToken}
macros:
  search:
    params: [q, index]
    defaults: { index: records }
    match: "the operator searches for {q}"
    bind: { q: "${q}", index: "${index}" }
    steps:
      - ref: admin.search
  ping:
    match: "the operator checks health"
    steps:
      - hurl: |
          GET ${url:base}/health
          HTTP 200
"#;

const MIXED_FEATURE: &str = "Feature: F\n\
     \x20 Scenario: S\n    When the operator searches for \"Acme\"\n\
     \x20 Scenario: T\n    When the operator checks health\n";

/// The newest run record's raw JSONL.
fn latest_events(cwd: &Path) -> String {
    let mut runs: Vec<PathBuf> = std::fs::read_dir(cwd.join(".proef-runs"))
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .collect();
    runs.sort();
    std::fs::read_to_string(runs.pop().unwrap().join("events.jsonl")).unwrap()
}

/// Every `step_finished` in the record, as parsed JSON.
fn step_events(jsonl: &str) -> Vec<serde_json::Value> {
    jsonl
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|event| event["event"] == "step_finished")
        .collect()
}

/// A `ref:` step records the fragment it ran, qualified `file#name` — and an
/// inline step records no such key at all.
///
/// The absence half is the ADR-0008 guarantee, not a detail: the field is
/// additive, so every stream written before fragments existed must still parse
/// and still compare equal to one written today by a suite that uses none.
#[test]
fn a_step_records_the_fragment_it_ran_and_inline_steps_record_none() {
    let fixture = Fixture::start().unwrap();
    let dir = project(FAILING_CORPUS, MIXED_PACK);
    std::fs::write(dir.path().join("tests/features/a.feature"), MIXED_FEATURE).unwrap();

    proef_in(dir.path(), &fixture)
        .args(["test"])
        .assert()
        .code(1);

    let steps = step_events(&latest_events(dir.path()));
    let refd = steps
        .iter()
        .find(|s| s["step"]["text"].as_str() == Some("the operator searches for \"Acme\""))
        .expect("the ref: step must appear in the record");
    assert_eq!(
        refd["fragment"].as_str(),
        Some("tests/hurl/admin.hurl#admin.search"),
        "a ref: step must name its fragment, file-qualified: {refd}"
    );

    let inline = steps
        .iter()
        .find(|s| s["step"]["text"].as_str() == Some("the operator checks health"))
        .expect("the inline step must appear in the record");
    assert!(
        inline.get("fragment").is_none(),
        "an inline step has no fragment, and the key must be absent rather \
         than null — pre-fragment streams are byte-unchanged: {inline}"
    );
}

/// `proef explain` names the fragment file a failure came from.
///
/// ADR-0018 accepted "a test spans three files" only on the condition that
/// tooling earn it back. Go-to-definition covers the authoring side; this is
/// the post-mortem side, and without it a reader has the Gherkin line and the
/// pack but not the request that actually failed.
#[test]
fn explain_names_the_fragment_a_failed_step_came_from() {
    let fixture = Fixture::start().unwrap();
    let dir = project(FAILING_CORPUS, PACK);

    proef_in(dir.path(), &fixture)
        .args(["test"])
        .assert()
        .code(1);

    let assert = proef_in(dir.path(), &fixture)
        .arg("explain")
        .assert()
        .code(0);
    let out = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    assert!(
        out.contains("via tests/hurl/admin.hurl#admin.search"),
        "explain must point at the fragment file, in the spelling `ref:` \
         accepts:\n{out}"
    );
}
