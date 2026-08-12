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

/// ADR-0007's caps bound the request that actually runs, so the file it was
/// written in cannot decide whether they apply. They used to: the value scan sat
/// inside the inline-only linter, so this exact `[Options]` block exited 2 as a
/// `hurl:` step and exited **0** — "dry-run OK, 0 warning(s)" — behind a `ref:`,
/// then went verbatim into the executed input.
///
/// This is the one bypass worth an end-to-end test rather than a unit one: hurl
/// has no cancellation, so an infinite retry leaves the watchdog abandoning a
/// thread it cannot stop, and the unit tests reach the check through a stub
/// scanner instead of the real parser that reads a `.hurl` file.
#[test]
fn a_fragments_option_values_are_capped_like_an_inline_blocks() {
    for (line, code) in [
        ("retry: -1", "proef::pack::retry_not_finite"),
        ("repeat: -1", "proef::pack::retry_not_finite"),
        ("delay: 99999999", "proef::pack::delay_unbounded"),
    ] {
        let hurl =
            format!("# @proef admin.search\nGET {{{{base}}}}/x\n[Options]\n{line}\nHTTP 200\n");
        let pack = "macros:\n  searchTasks:\n    match: the operator searches tasks\n    \
                    steps:\n      - ref: admin.search\n";
        let feature = "Feature: F\n  Scenario: S\n    When the operator searches tasks\n";
        let dir = project(&hurl, pack);
        std::fs::write(dir.path().join("tests/features/a.feature"), feature).unwrap();
        let assert = Command::cargo_bin("proef")
            .unwrap()
            .current_dir(dir.path())
            .env("NO_COLOR", "1")
            .env("PROEF_BASE_URL", "http://127.0.0.1:1")
            .args(["test", "--dry-run"])
            .assert()
            .code(2);
        let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
        assert!(stderr.contains(code), "`{line}` in a fragment: {stderr}");
        // The pack author cannot fix what they cannot find: the diagnostic
        // anchors on their `ref:` line, so it has to name the other file.
        assert!(stderr.contains("admin.hurl"), "`{line}`: {stderr}");
    }
}

/// The listing exists for the two questions nothing else could answer: which
/// annotated fragments nothing reaches, and which entries carry no annotation at
/// all. Both were silent — a missing annotation produces a green run and a
/// silently absent test.
#[test]
fn the_listing_names_both_ways_a_fragment_dies() {
    let hurl = "# @proef api.used\nGET {{base}}/a\nHTTP 200\n\n\
                # @proef api.deadNoMacro\nGET {{base}}/b\nHTTP 200\n\n\
                # @proef api.deadViaMacro\nGET {{base}}/c\nHTTP 200\n\n\
                GET {{base}}/forgot\nHTTP 200\n";
    let pack = "macros:\n  searchTasks:\n    match: the operator searches tasks\n    \
                steps:\n      - ref: api.used\n        bind: { base: \"${url:base}\" }\n  \
                orphanMacro:\n    match: nobody says this\n    steps:\n      \
                - ref: api.deadViaMacro\n        bind: { base: \"${url:base}\" }\n";
    let dir = project(hurl, pack);
    std::fs::write(
        dir.path().join("tests/features/a.feature"),
        "Feature: F\n  Scenario: S\n    When the operator searches tasks\n",
    )
    .unwrap();

    let assert = Command::cargo_bin("proef")
        .unwrap()
        .current_dir(dir.path())
        .env("NO_COLOR", "1")
        .env("PROEF_BASE_URL", "http://127.0.0.1:1")
        .arg("fragments")
        .assert()
        .code(0);
    let out = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();

    assert!(out.contains("UNREFERENCED"), "{out}");
    assert!(out.contains("api.deadNoMacro"), "{out}");
    // The dangerous one: the *macro* warning already fires, so this reads as
    // covered unless the fragment is named directly, with the reason.
    assert!(out.contains("UNREACHABLE"), "{out}");
    assert!(out.contains("orphanMacro"), "{out}");
    // No name to list it by, so it is listed by line.
    assert!(out.contains("UNANNOTATED"), "{out}");
    // The denominator, without which neither death mode is noticeable.
    assert!(
        out.contains("4 entries · 3 annotated · 1 unannotated · 2 never run"),
        "{out}"
    );

    // `--check` gates the first class; the second needs opting in, because an
    // unannotated entry is inert by design and only a porting team reads it as
    // unfinished.
    let mut check = Command::cargo_bin("proef").unwrap();
    check
        .current_dir(dir.path())
        .env("NO_COLOR", "1")
        .env("PROEF_BASE_URL", "http://127.0.0.1:1")
        .args(["fragments", "--check"])
        .assert()
        .code(1);
}

/// A `bind:` key nothing reads was the one authoring mistake in the fragment
/// path with no signal at all: the typo'd name simply never arrives, and the run
/// stays green. The did-you-mean matters more here than usual, because the
/// mistake is in the *pack* while the only previous symptom pointed at the
/// *fragment*.
#[test]
fn a_bind_key_no_fragment_reads_is_refused_with_a_suggestion() {
    let hurl = "# @proef api.used\nGET {{base}}/a?t={{token}}\nHTTP 200\n";
    let pack = "macros:\n  searchTasks:\n    match: the operator searches tasks\n    \
                steps:\n      - ref: api.used\n        bind: { base: \"${url:base}\", \
                token: \"x\", toekn: \"y\" }\n";
    let stderr = dry_run_error(hurl, pack);
    assert!(stderr.contains("proef::pack::unread_bind_key"), "{stderr}");
    assert!(stderr.contains("did you mean `token`?"), "{stderr}");
}

/// Two entries in one file collide by name like any other pair — but the
/// cross-file remedy is wrong there: `file.hurl#name` qualifies by file and
/// cannot separate two entries inside one. Annotating a corpus adds many names
/// to few files, so this is the likely collision, not the exotic one.
#[test]
fn a_same_file_duplicate_says_so_and_offers_a_remedy_that_works() {
    let hurl = format!("{CORPUS}\n# @proef admin.search\nGET {{{{base}}}}/other\nHTTP 200\n");
    let stderr = dry_run_error(&hurl, PACK);
    assert!(stderr.contains("declared twice in"), "{stderr}");
    assert!(
        !stderr.contains("file.hurl#name"),
        "the qualifier cannot disambiguate within one file: {stderr}"
    );
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

/// A fragment may answer its own question. `[Options] variable:` is how a
/// corpus file stays runnable on its own — fewer variables to pass in — and
/// ADR-0018's premise is that proef runs the file the backend team already has.
/// Proef once refused exactly these files, reporting the self-supplied name as
/// unsupplied.
#[test]
fn a_variable_the_fragment_supplies_itself_needs_no_binding() {
    let fixture = Fixture::start().unwrap();
    let hurl = CORPUS.replace("[Query]", "[Options]\nvariable: index=records\n[Query]");
    // The pack no longer says anything about `index`; the file does.
    let pack = PACK.replace(
        r#"bind: { q: "${q}", index: "${index}" }"#,
        r#"bind: { q: "${q}" }"#,
    );
    let dir = project(&hurl, &pack);

    proef_in(dir.path(), &fixture)
        .args(["test", "tests/features"])
        .assert()
        .code(0);
}

/// The other edge of the same rule. Both a `bind:` and the fragment's own line
/// reach the entry as `variable: index=`, where hurl takes the last — the
/// fragment's — so the bound value would never reach the request. Refused
/// rather than resolved, exactly as a doubly-declared `retry:` is.
#[test]
fn a_variable_the_fragment_supplies_and_the_pack_binds_is_refused() {
    let hurl = CORPUS.replace("[Query]", "[Options]\nvariable: index=records\n[Query]");
    let stderr = dry_run_error(&hurl, PACK);
    assert!(
        stderr.contains("proef::pack::option_declared_twice"),
        "{stderr}"
    );
    assert!(
        stderr.contains("index") && stderr.contains("admin.hurl"),
        "it must name the variable and the file it came from: {stderr}"
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

/// The CI artifacts name the fragment too — `JUnit` here, standing in for the
/// whole `RunSummary`-derived family (job summary, `::error` annotations, TAP,
/// console).
///
/// Those read `StepOutcome`, not the event stream, so they are fed by a
/// *second* copy of the provenance that every engine fills in itself. Without
/// an end-to-end assertion, blanking that copy in the engine leaves the whole
/// suite green while every CI artifact silently loses the fragment — the half
/// of the feature a reader who cannot grep the checkout depends on.
#[test]
fn junit_names_the_fragment_a_failure_came_from() {
    let fixture = Fixture::start().unwrap();
    let dir = project(FAILING_CORPUS, PACK);
    let junit = dir.path().join("report.junit.xml");

    proef_in(dir.path(), &fixture)
        .args(["test", "--junit", &junit.display().to_string()])
        .assert()
        .code(1);

    let xml = std::fs::read_to_string(&junit).unwrap();
    assert!(
        xml.contains("(via tests/hurl/admin.hurl#admin.search)"),
        "the JUnit failure message must name the fragment: {xml}"
    );
}

/// A corpus is **foreign by design** — CONFIG sells "pointing at a corpus you
/// did not write costs nothing" — so one unreadable file must not take down
/// commands that never look at fragments. It used to exit 3 from every command,
/// `flows` included.
#[test]
fn an_unreadable_corpus_file_never_sinks_its_siblings() {
    let dir = project(CORPUS, PACK);
    std::fs::write(
        dir.path().join("tests/hurl/binary.hurl"),
        [0xff, 0xfe, 0x00, 0x01],
    )
    .unwrap();

    // A suite whose packs contain no `ref:` never uses the corpus, so an
    // unreadable file in it must cost exactly nothing — the promise CONFIG
    // makes. This is the half that used to exit 3 from every command.
    let inert = project(
        CORPUS,
        "macros:\n  noop:\n    match: \"the operator searches for {q}\"\n    params: [q]\n    \
         steps:\n      - hurl: |\n          GET ${url:base}/x\n          HTTP 200\n",
    );
    std::fs::write(
        inert.path().join("tests/hurl/binary.hurl"),
        [0xff, 0xfe, 0x00, 0x01],
    )
    .unwrap();
    Command::cargo_bin("proef")
        .unwrap()
        .current_dir(inert.path())
        .env("NO_COLOR", "1")
        .env("PROEF_BASE_URL", "http://127.0.0.1:1")
        .env("PROEF_SECRET_APITOKEN", API_TOKEN)
        .args(["flows", "tests/features"])
        .assert()
        .code(0);

    // The pack does `ref:` the corpus, so now it is reported — and the readable
    // sibling still resolved, or this would be an `unknown_ref` instead.
    let mut cmd = Command::cargo_bin("proef").unwrap();
    let assert = cmd
        .current_dir(dir.path())
        .env("NO_COLOR", "1")
        .env("PROEF_BASE_URL", "http://127.0.0.1:1")
        .env("PROEF_SECRET_APITOKEN", API_TOKEN)
        .args(["test", "--dry-run"])
        .assert()
        .code(2);
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
    assert!(
        stderr.contains("proef::pack::unreadable_fragment_file"),
        "{stderr}"
    );
    assert!(
        !stderr.contains("unknown_ref"),
        "the readable sibling must still load: {stderr}"
    );
}

/// `#` separates a file from a fragment in `ref: file.hurl#name`, so a name
/// carrying one could be declared and never referenced — the lookup split on it
/// and then suggested the exact spelling that had just failed.
#[test]
fn an_annotation_name_containing_a_hash_is_refused() {
    let hurl = CORPUS.replace("# @proef admin.search", "# @proef admin#search");
    let stderr = dry_run_error(&hurl, PACK);
    assert!(stderr.contains("proef::pack::bad_annotation"), "{stderr}");
    assert!(stderr.contains("could never be referenced"), "{stderr}");
}

/// `--add-to` writes a yaml modeline and drops the pack schema beside the file.
/// Neither means anything to a `.hurl` corpus, and writing one violates
/// ADR-0018's "fragment files are inputs proef never writes". `fmt` learned this
/// predicate; this writer had not.
#[test]
fn schema_add_to_refuses_a_fragment_file() {
    let dir = project(CORPUS, PACK);
    let fragment = dir.path().join("tests/hurl/admin.hurl");
    let before = std::fs::read_to_string(&fragment).unwrap();

    Command::cargo_bin("proef")
        .unwrap()
        .current_dir(dir.path())
        .env("NO_COLOR", "1")
        .args(["schema", "--add-to", "tests/hurl/admin.hurl"])
        .assert()
        .code(2);

    assert_eq!(
        std::fs::read_to_string(&fragment).unwrap(),
        before,
        "the corpus file must be byte-identical"
    );
    assert!(
        !dir.path()
            .join("tests/hurl/proef-pack.schema.json")
            .exists(),
        "no schema may be dropped into the corpus"
    );
}

/// ADR-0018 promises that pointing at a corpus you did not write costs nothing.
/// The ADR-0007 value caps read the entry's text, and while they had no
/// `[Options]` gate a `[Query]` parameter, form field or lowercase header named
/// `retry` was a hard error — proef rejecting somebody else's file for a line
/// hurl treats as data. The gate has always existed on the inline half; the
/// value scan inherited a laxer rule that was invisible while it only read
/// proef's own YAML.
#[test]
fn a_query_parameter_named_like_an_option_is_not_an_option() {
    let hurl =
        "# @proef admin.search\nGET {{base}}/x\n[Query]\nretry: -1\ndelay: 99999999\nHTTP 200\n";
    let pack = "macros:\n  searchTasks:\n    match: the operator searches tasks\n    \
                steps:\n      - ref: admin.search\n        bind: { base: \"${url:base}\" }\n";
    let dir = project(hurl, pack);
    std::fs::write(
        dir.path().join("tests/features/a.feature"),
        "Feature: F\n  Scenario: S\n    When the operator searches tasks\n",
    )
    .unwrap();
    Command::cargo_bin("proef")
        .unwrap()
        .current_dir(dir.path())
        .env("NO_COLOR", "1")
        .env("PROEF_BASE_URL", "http://127.0.0.1:1")
        .args(["test", "--dry-run"])
        .assert()
        .code(0);
}

/// One typo'd `ref:` is one mistake. It used to produce three diagnostics: the
/// correct `unknown_ref`, plus a macro-scope and a pack-scope `unread_bind_key`
/// telling the author to delete a `bind:` that was never wrong — because the
/// readable set is built from the fragments a `ref:` resolves to, and an
/// unresolved one silently contributes nothing. A conclusion drawn from a scope
/// known to be missing pieces is not a finding.
#[test]
fn an_unresolved_ref_does_not_make_its_bindings_look_unread() {
    let hurl = "# @proef admin.search\nGET {{base}}/x?q={{q}}\nHTTP 200\n";
    let pack = "bind:\n  q: laptop\nmacros:\n  searchTasks:\n    match: the operator searches tasks\n    \
                bind: { q: laptop }\n    steps:\n      - ref: typo.name\n";
    let stderr = dry_run_error(hurl, pack);
    assert!(stderr.contains("proef::pack::unknown_ref"), "{stderr}");
    assert!(
        !stderr.contains("unread_bind_key"),
        "the bindings are fine; the `ref:` is the mistake — {stderr}"
    );
}
