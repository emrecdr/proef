//! End-to-end stdio lifecycle for `proef lsp`: the ONLY test that exercises the
//! real lsp-server `IoThreads` (the in-process tests use `Connection::memory`, whose
//! `io_threads` are None). Proves the server process actually EXITS after
//! shutdown/exit — the regression guard for the writer-thread leak.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::{BufRead, BufReader, Write};
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
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());

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
            let _ = child.kill();
            panic!("proef lsp did not exit within 10s after shutdown/exit — writer thread leaked");
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    assert!(
        status.success(),
        "proef lsp exited with {status:?}, expected success"
    );
}
