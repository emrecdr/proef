//! End-to-end stdio lifecycle for `proef lsp`: the ONLY test that exercises the
//! real lsp-server `IoThreads` (the in-process tests use `Connection::memory`, whose
//! `io_threads` are None). Proves the server process actually EXITS after
//! shutdown/exit — the regression guard for the writer-thread leak.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Frame a JSON string as an LSP message (`Content-Length` header + body).
fn write_msg(stdin: &mut impl Write, json: &str) {
    write!(stdin, "Content-Length: {}\r\n\r\n{}", json.len(), json).unwrap();
    stdin.flush().unwrap();
}

/// Read one LSP-framed message body from the stream, returning its JSON text.
fn read_msg(reader: &mut impl BufRead) -> String {
    let mut len = 0usize;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            break; // end of headers
        }
        if let Some(rest) = trimmed.strip_prefix("Content-Length:") {
            len = rest.trim().parse().unwrap();
        }
    }
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body).unwrap();
    String::from_utf8(body).unwrap()
}

#[test]
fn stdio_server_exits_after_shutdown_and_exit() {
    let bin = assert_cmd::cargo::cargo_bin("proef");
    let dir = tempfile::tempdir().unwrap(); // empty suite: no recompute is triggered
    let mut child = Command::new(bin)
        .arg("lsp")
        .current_dir(dir.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut stderr = child.stderr.take().unwrap();

    // initialize → expect a result
    write_msg(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}"#,
    );
    let init_resp = read_msg(&mut stdout);
    assert!(
        init_resp.contains("\"id\":1"),
        "initialize response: {init_resp}"
    );

    // initialized notification
    write_msg(
        &mut stdin,
        r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#,
    );
    // shutdown → expect an ack
    write_msg(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":2,"method":"shutdown","params":null}"#,
    );
    let shutdown_resp = read_msg(&mut stdout);
    assert!(
        shutdown_resp.contains("\"id\":2"),
        "shutdown response: {shutdown_resp}"
    );

    // exit notification → the process must now terminate on its own
    write_msg(
        &mut stdin,
        r#"{"jsonrpc":"2.0","method":"exit","params":null}"#,
    );
    drop(stdin); // close our end of the pipe

    // Watchdog: poll for exit; kill + fail if the writer thread leaked the loop.
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            // The process is dying (or dead) now, so its stderr write end is
            // closing — safe to drain to EOF here without risking a deadlock
            // against a still-running child.
            let _ = child.kill();
            let mut captured = String::new();
            let _ = stderr.read_to_string(&mut captured);
            panic!(
                "proef lsp did not exit within 10s after shutdown/exit — writer thread leaked\nstderr:\n{captured}"
            );
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    if !status.success() {
        // The child has already exited, so reading its stderr to EOF here
        // cannot block on a still-running process.
        let mut captured = String::new();
        let _ = stderr.read_to_string(&mut captured);
        panic!("proef lsp exited with {status:?}, expected success\nstderr:\n{captured}");
    }
}

/// The server adopts the workspace root the client announces.
///
/// The root is resolved at the process edge, before the handshake, from the
/// process working directory — so a server launched anywhere but the project
/// analysed the wrong tree. `nvim ~/proj/x.feature` from `$HOME` rooted the
/// analyser at `$HOME`. The client is the authority on where its workspace is.
///
/// Proven by diagnostics: the CWD holds a suite that binds cleanly, the
/// announced workspace holds one that does not. A diagnostic for the announced
/// workspace's file can only come from having rooted there.
/// A `file:` URI for `path`, valid on both platform families.
///
/// Windows paths use `\\`, which is an invalid escape inside a JSON string —
/// interpolating one straight into a request produces a message the server
/// cannot parse, so it dies during the handshake and the test sees an empty
/// reply rather than a useful failure. Separators become `/`, and a drive
/// letter gets the third slash (`file:///C:/...`) the URI form requires.
///
/// The `\\?\` verbatim prefix is stripped first. `std::fs::canonicalize` returns
/// one on Windows, and left in place it survives the separator swap as a leading
/// `//?/`, which then takes the `starts_with('/')` branch and yields a
/// four-slash `file:////?/C:/…` no client can resolve — the workspace root
/// silently points nowhere and every request answers `null`. Production never
/// produces a verbatim path (`disk_provider` and `lsp` both keep source names
/// uncanonicalized on purpose), so this is the test's own hazard to clear.
fn file_uri(path: &std::path::Path) -> String {
    let raw = path.to_string_lossy();
    let text = raw.strip_prefix(r"\\?\").unwrap_or(&raw).replace('\\', "/");
    if text.starts_with('/') {
        format!("file://{text}")
    } else {
        format!("file:///{text}")
    }
}

/// [`file_uri`] is pure string work, so its Windows-shaped inputs are checked on
/// **every** platform. The alternative is what already happened once: the
/// verbatim-prefix bug was invisible on macOS and only surfaced on Windows CI,
/// as a `null` answer three requests later with nothing pointing at the URI.
#[test]
fn file_uri_renders_windows_shapes_a_client_can_resolve() {
    use std::path::PathBuf;

    // `std::fs::canonicalize` returns this shape on Windows.
    assert_eq!(
        file_uri(&PathBuf::from(r"\\?\C:\proj\tests\hurl")),
        "file:///C:/proj/tests/hurl",
        "a verbatim prefix must not survive into the URI"
    );
    assert_eq!(
        file_uri(&PathBuf::from(r"C:\proj\a.hurl")),
        "file:///C:/proj/a.hurl",
        "a drive letter takes the third slash"
    );
    assert_eq!(
        file_uri(&PathBuf::from("/proj/a.hurl")),
        "file:///proj/a.hurl",
        "a unix path keeps exactly three slashes"
    );
}

#[test]
fn the_server_roots_at_the_workspace_the_client_announces() {
    let bin = assert_cmd::cargo::cargo_bin("proef");
    let cwd = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();

    // The directory the process is launched in: a suite with nothing wrong.
    std::fs::create_dir_all(cwd.path().join("packs")).unwrap();
    std::fs::write(cwd.path().join("ok.feature"), "Feature: F\n").unwrap();

    // The workspace the client announces: a step no pack binds.
    std::fs::create_dir_all(elsewhere.path().join("packs")).unwrap();
    std::fs::write(
        elsewhere.path().join("broken.feature"),
        "Feature: F\n  Scenario: S\n    When nothing binds this sentence\n",
    )
    .unwrap();
    std::fs::write(elsewhere.path().join("packs/p.yaml"), "macros: {}\n").unwrap();

    let mut child = Command::new(bin)
        .arg("lsp")
        .current_dir(cwd.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());

    // Not canonicalized: the server walks the root it is given and reports
    // paths joined from it, so client and server agree as long as both use the
    // same spelling. Canonicalizing would also introduce Windows extended-length
    // prefixes (\\?\C:\...), which are not URI-shaped.
    let announced = elsewhere.path().to_path_buf();
    let uri = file_uri(&announced);
    write_msg(
        &mut stdin,
        &format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"capabilities":{{}},"workspaceFolders":[{{"uri":"{uri}","name":"w"}}]}}}}"#
        ),
    );
    let init_resp = read_msg(&mut stdout);
    assert!(init_resp.contains("\"id\":1"), "{init_resp}");
    write_msg(
        &mut stdin,
        r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#,
    );

    // Open the announced workspace's broken file; its diagnostics prove the root.
    let doc_uri = file_uri(&announced.join("broken.feature"));
    write_msg(
        &mut stdin,
        &format!(
            r#"{{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{{"textDocument":{{"uri":"{doc_uri}","languageId":"gherkin","version":1,"text":"Feature: F\n  Scenario: S\n    When nothing binds this sentence\n"}}}}}}"#
        ),
    );

    // Read on a thread. `read_msg` blocks, so polling a deadline around it only
    // works while messages keep arriving — and the failure being guarded against
    // is precisely "no diagnostic ever comes", which would hang rather than
    // fail. A channel turns that into a timeout.
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        loop {
            let msg = read_msg(&mut stdout);
            if tx.send(msg).is_err() {
                break;
            }
        }
    });

    let deadline = Instant::now() + Duration::from_secs(20);
    let mut saw = false;
    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        match rx.recv_timeout(remaining) {
            Ok(msg) if msg.contains("publishDiagnostics") && msg.contains("unbound_step") => {
                saw = true;
                break;
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    assert!(
        saw,
        "the announced workspace's file must be analysed — the server rooted elsewhere"
    );

    write_msg(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":2,"method":"shutdown","params":null}"#,
    );
    write_msg(&mut stdin, r#"{"jsonrpc":"2.0","method":"exit"}"#);
    drop(stdin);
    let _ = child.wait();
}

/// Go-to-definition on a `ref:` line reaches the `.hurl` file, with the
/// fragment root coming from a **real `proef.toml`** rather than a test double.
///
/// This is the seam the unit tests cannot cover: `proef-lsp`'s own tests inject
/// absolute source names through `FakeDisk`, so they never exercise
/// `ProjectConfig::fragments()` → `DiskSourceProvider` → `name_to_url`. That gap
/// let a regression ship — shortening the configured root to a cwd-relative
/// spelling (for the sake of portable run records) made every fragment name
/// relative, and `name_to_url` yields `None` for those, so this request returned
/// null while the suite still ran green.
#[test]
fn definition_on_a_ref_line_resolves_through_the_configured_fragment_root() {
    let bin = assert_cmd::cargo::cargo_bin("proef");
    let dir = tempfile::tempdir().unwrap();
    // Canonicalized deliberately: on macOS a tempdir is `/var/...` whose real
    // path is `/private/var/...`, so an uncanonicalized root disagrees with the
    // server's own `current_dir()` and any cwd-relative path handling silently
    // no-ops — the test would pass without ever reaching the behaviour. A real
    // editor announces the same path the process runs in.
    let root = &std::fs::canonicalize(dir.path()).unwrap();
    std::fs::create_dir_all(root.join("tests/features/packs")).unwrap();
    std::fs::create_dir_all(root.join("tests/hurl")).unwrap();
    std::fs::write(
        root.join("proef.toml"),
        "[run]\nsuite = \"tests/features\"\nfragments = \"tests/hurl\"\n",
    )
    .unwrap();
    std::fs::write(
        root.join("tests/hurl/admin.hurl"),
        "# corpus header\n# @proef admin.search\nGET http://127.0.0.1:1/x\nHTTP 200\n",
    )
    .unwrap();
    let pack = "macros:\n  search:\n    match: \"the operator searches\"\n    steps:\n      - ref: admin.search\n";
    std::fs::write(root.join("tests/features/packs/api.yaml"), pack).unwrap();
    std::fs::write(
        root.join("tests/features/a.feature"),
        "Feature: F\n  Scenario: S\n    When the operator searches\n",
    )
    .unwrap();

    let mut child = Command::new(bin)
        .arg("lsp")
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());

    let uri = file_uri(root);
    write_msg(
        &mut stdin,
        &format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"capabilities":{{}},"workspaceFolders":[{{"uri":"{uri}","name":"w"}}]}}}}"#
        ),
    );
    assert!(read_msg(&mut stdout).contains("\"id\":1"));
    write_msg(
        &mut stdin,
        r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#,
    );

    // Open the pack, then ask where its `ref:` points. Line 4 is
    // "      - ref: admin.search"; character 20 sits inside the target name.
    let pack_uri = file_uri(&root.join("tests/features/packs/api.yaml"));
    let escaped = pack.replace('\n', "\\n").replace('"', "\\\"");
    write_msg(
        &mut stdin,
        &format!(
            r#"{{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{{"textDocument":{{"uri":"{pack_uri}","languageId":"yaml","version":1,"text":"{escaped}"}}}}}}"#
        ),
    );
    write_msg(
        &mut stdin,
        &format!(
            r#"{{"jsonrpc":"2.0","id":2,"method":"textDocument/definition","params":{{"textDocument":{{"uri":"{pack_uri}"}},"position":{{"line":4,"character":20}}}}}}"#
        ),
    );

    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        loop {
            let msg = read_msg(&mut stdout);
            if tx.send(msg).is_err() {
                break;
            }
        }
    });

    let deadline = Instant::now() + Duration::from_secs(20);
    let mut answer = None;
    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        match rx.recv_timeout(remaining) {
            Ok(msg) if msg.contains("\"id\":2") => {
                answer = Some(msg);
                break;
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }

    let answer = answer.expect("the definition request must be answered");
    assert!(
        answer.contains("admin.hurl"),
        "go-to-definition must land in the fragment file, not return null: {answer}"
    );

    write_msg(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":3,"method":"shutdown","params":null}"#,
    );
    write_msg(&mut stdin, r#"{"jsonrpc":"2.0","method":"exit"}"#);
    drop(stdin);
    let _ = child.wait();
}
