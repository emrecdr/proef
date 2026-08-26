//! Scripted JSON-RPC integration tests: drive the server over an in-memory
//! connection with a fake disk provider, and assert the published diagnostics
//! and go-to-definition responses. No real filesystem and no real engine — a
//! `hurl` step kind with no `validate` probe is enough to load the pack and
//! bind the feature.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use lsp_server::{Connection, Message, Notification, Request, RequestId};
use lsp_types::{
    Definition, DefinitionParams, DefinitionResponse, DidOpenTextDocumentParams, InitializeParams,
    InitializedParams, LanguageKind, Location, PartialResultParams, Position,
    PublishDiagnosticsParams, TextDocumentIdentifier, TextDocumentItem, TextDocumentPositionParams,
    Uri, WorkDoneProgressParams,
};
use proef_core::engine::StepKindSpec;
use proef_core::provider::{ProviderError, SourceProvider};
use proef_lsp::documents::name_to_url;
use proef_lsp::{ServerConfig, Transport, run};

/// A disk provider seeded from an in-memory map — no real filesystem.
struct FakeDisk {
    features: Vec<String>,
    packs: Vec<String>,
    fragments: Vec<String>,
    files: BTreeMap<String, Arc<str>>,
}
impl SourceProvider for FakeDisk {
    fn discover_features(&self) -> Result<Vec<String>, ProviderError> {
        Ok(self.features.clone())
    }
    fn discover_packs(&self) -> Result<Vec<String>, ProviderError> {
        Ok(self.packs.clone())
    }
    fn discover_fragments(&self) -> Result<Vec<String>, ProviderError> {
        Ok(self.fragments.clone())
    }
    fn read(&self, name: &str) -> Result<Arc<str>, ProviderError> {
        self.files
            .get(name)
            .cloned()
            .ok_or_else(|| ProviderError(format!("no {name}")))
    }
}

/// A [`FakeDisk`] that counts every `read`, so a test can assert how many times
/// the pipeline went back to the provider — the deterministic stand-in for
/// "did this recompute?" that the flake rule prefers over timing.
struct CountingDisk {
    inner: FakeDisk,
    reads: Arc<AtomicUsize>,
}
impl SourceProvider for CountingDisk {
    fn discover_features(&self) -> Result<Vec<String>, ProviderError> {
        self.inner.discover_features()
    }
    fn discover_packs(&self) -> Result<Vec<String>, ProviderError> {
        self.inner.discover_packs()
    }
    fn discover_fragments(&self) -> Result<Vec<String>, ProviderError> {
        self.inner.discover_fragments()
    }
    fn read(&self, name: &str) -> Result<Arc<str>, ProviderError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.inner.read(name)
    }
}

/// A stand-in fragment scanner: `@name` opens a fragment, `?var` declares a
/// placeholder. Keeps the test off the real hurl parser while still exercising
/// the seam the LSP reads fragments through.
///
/// The `Result` is never `Err` here, but the signature is fixed by
/// `engine::FragmentScanner`'s fn-pointer type — it cannot be narrowed.
#[allow(clippy::unnecessary_wraps)]
fn fake_scan(
    text: &str,
) -> Result<proef_core::engine::ScannedFile, proef_core::engine::FragmentScanError> {
    let mut out: Vec<proef_core::engine::ScannedFragment> = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if let Some(name) = line.strip_prefix('@') {
            out.push(proef_core::engine::ScannedFragment {
                name: name.to_owned(),
                text: format!("GET http://x/{name}\nHTTP 200\n"),
                line: index + 1,
                placeholders: Vec::new(),
                declared_options: Vec::new(),
                supplied_variables: Vec::new(),
            });
        } else if let Some(last) = out.last_mut() {
            if let Some(read) = line.strip_prefix('?') {
                last.placeholders.push(read.to_owned());
            } else if let Some(supplied) = line.strip_prefix('=') {
                // `[Options] variable:` — read *and* supplied by the entry.
                last.placeholders.push(supplied.to_owned());
                last.supplied_variables.push(supplied.to_owned());
            }
        }
    }
    Ok(proef_core::engine::ScannedFile {
        fragments: out,
        unannotated: Vec::new(),
    })
}

/// Spawns the server over an in-memory connection and returns the client end
/// with its join handle.
///
/// Every test needs the same `ServerConfig` and differs only in its provider and
/// its step kinds, so only those are parameters. The root is always `/suite`:
/// discovery is the provider's own concern (`recompute` explicitly ignores the
/// root), and every provider here is a fake that answers from a map.
fn spawn(
    disk: impl SourceProvider + Send + 'static,
    kinds: Vec<StepKindSpec>,
) -> (Connection, std::thread::JoinHandle<()>) {
    let (server_conn, client) = Connection::memory();
    let server = std::thread::spawn(move || {
        run(ServerConfig {
            transport: Transport::InMemory(server_conn),
            root: PathBuf::from("/suite"),
            disk: Box::new(disk),
            kinds,
            kind_to_engine: BTreeMap::from([("hurl".to_owned(), "hurl".to_owned())]),
            env: BTreeMap::new(),
            config_vars: BTreeMap::new(),
            debounce: Duration::ZERO,
            resolve_root: None,
        })
        .unwrap();
    });
    (client, server)
}

/// The `hurl` step kind with no engine behind it — enough for a pack to load
/// without an `unknown_step_kind` diagnostic, which is all most tests need.
fn hurl_kinds() -> Vec<StepKindSpec> {
    vec![StepKindSpec {
        prefix: "hurl",
        schema: "true",
        validate: None,
        fragments: None,
        options: None,
    }]
}

/// The same kind, claiming `.hurl` fragments through [`fake_scan`] — for the
/// tests that exercise `ref:` and `bind:`.
fn hurl_kinds_with_fragments() -> Vec<StepKindSpec> {
    vec![StepKindSpec {
        fragments: Some(proef_core::engine::FragmentSupport {
            ext: "hurl",
            scan: fake_scan,
            template_reads: |_| Vec::new(),
        }),
        ..hurl_kinds().remove(0)
    }]
}

/// A `textDocument/codeAction` probe for one document: ask at a cursor point,
/// get the actions back. Returned as a closure so a test reads as a sequence of
/// cursor positions, which is the only thing that varies between them.
fn code_action_probe<'a>(
    client: &'a Connection,
    url: &'a Uri,
) -> impl Fn(i32, u32, u32) -> Vec<lsp_types::CodeActionResponse> + 'a {
    move |id: i32, line: u32, character: u32| {
        let point = Position { line, character };
        client
            .sender
            .send(Message::Request(Request {
                id: RequestId::from(id),
                method: "textDocument/codeAction".to_owned(),
                params: serde_json::to_value(lsp_types::CodeActionParams {
                    text_document: TextDocumentIdentifier { uri: url.clone() },
                    range: lsp_types::Range {
                        start: point,
                        end: point,
                    },
                    // Empty, deliberately: the client echoes the diagnostics it
                    // holds, and none of them can carry a fix. The analysis is
                    // the authority, so the handler must not need this.
                    context: lsp_types::CodeActionContext {
                        diagnostics: Vec::new(),
                        only: None,
                        trigger_kind: None,
                    },
                    work_done_progress_params: WorkDoneProgressParams::default(),
                    partial_result_params: PartialResultParams::default(),
                })
                .unwrap(),
            }))
            .unwrap();
        wait_for_response::<Option<Vec<lsp_types::CodeActionResponse>>>(
            client,
            &RequestId::from(id),
        )
        .unwrap_or_default()
    }
}

fn init(client: &Connection) {
    client
        .sender
        .send(Message::Request(Request {
            id: RequestId::from(1),
            method: "initialize".to_owned(),
            params: serde_json::to_value(InitializeParams::default()).unwrap(),
        }))
        .unwrap();
    let _ = client.receiver.recv().unwrap(); // initialize response
    client
        .sender
        .send(Message::Notification(Notification {
            method: "initialized".to_owned(),
            params: serde_json::to_value(InitializedParams {}).unwrap(),
        }))
        .unwrap();
}

/// Sends a `textDocument/didOpen` notification for `text` at `url`.
fn open(client: &Connection, url: &Uri, text: &str) {
    client
        .sender
        .send(Message::Notification(Notification {
            method: "textDocument/didOpen".to_owned(),
            params: serde_json::to_value(DidOpenTextDocumentParams {
                text_document: TextDocumentItem {
                    uri: url.clone(),
                    language_id: LanguageKind::Custom("gherkin".into()),
                    version: 1,
                    text: text.to_owned(),
                },
            })
            .unwrap(),
        }))
        .unwrap();
}

/// Waits for the next `textDocument/publishDiagnostics` notification,
/// regardless of which file it targets or whether it is empty — used to
/// know a recompute has happened at all.
fn wait_for_any_diagnostics(client: &Connection) -> PublishDiagnosticsParams {
    loop {
        let msg = client
            .receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("diagnostics timely");
        if let Message::Notification(n) = msg
            && n.method == "textDocument/publishDiagnostics"
        {
            return serde_json::from_value(n.params).unwrap();
        }
    }
}

/// Waits for the non-empty `publishDiagnostics` for `url` specifically —
/// built on [`wait_for_any_diagnostics`] so there is one receive loop.
fn wait_for_diagnostics(client: &Connection, url: &Uri) -> PublishDiagnosticsParams {
    loop {
        let p = wait_for_any_diagnostics(client);
        if &p.uri == url && !p.diagnostics.is_empty() {
            return p;
        }
    }
}

/// Waits for the `Message::Response` matching `id` and deserializes its result.
fn wait_for_response<T: serde::de::DeserializeOwned>(client: &Connection, id: &RequestId) -> T {
    loop {
        let msg = client
            .receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("response timely");
        if let Message::Response(resp) = msg
            && &resp.id == id
        {
            return serde_json::from_value(resp.response_result.unwrap()).unwrap();
        }
    }
}

/// Waits for the response `Message` with `id`, returning it raw so a test can
/// inspect either `result` or `error`.
fn wait_for_response_message(client: &Connection, id: &RequestId) -> Message {
    loop {
        let msg = client
            .receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("response timely");
        if let Message::Response(ref r) = msg
            && &r.id == id
        {
            return msg;
        }
    }
}

/// Runs the LSP `shutdown`/`exit` sequence and joins the server thread.
fn shutdown(client: &Connection, server: std::thread::JoinHandle<()>) {
    client
        .sender
        .send(Message::Request(Request {
            id: RequestId::from(99),
            method: "shutdown".to_owned(),
            params: serde_json::Value::Null,
        }))
        .unwrap();
    let _ = client.receiver.recv();
    client
        .sender
        .send(Message::Notification(Notification {
            method: "exit".to_owned(),
            params: serde_json::Value::Null,
        }))
        .unwrap();
    server.join().unwrap();
}

#[test]
fn open_unbound_step_publishes_the_expected_diagnostic() {
    let feature_name = native_abs("suite/case.feature");
    let pack_name = native_abs("suite/packs/broken.yaml");
    let mut files = BTreeMap::new();
    files.insert(feature_name.clone(), Arc::from(feature_text()));
    files.insert(
        pack_name.clone(),
        Arc::from(
            "macros:\n  search:\n    params: [term]\n    match: \"I search for {term}\"\n    steps:\n      - hurl: |\n          GET http://x\n",
        ),
    );

    let disk = FakeDisk {
        features: vec![feature_name.clone()],
        packs: vec![pack_name],
        fragments: Vec::new(),
        files,
    };

    // The `hurl` step kind is registered (with no `validate` probe) so the
    // pack loads without an `unknown_step_kind` diagnostic on it; the feature
    // step here is deliberately left unbound by the `serch`/`search` typo in
    // `feature_text()`, independent of kind registration.
    let (client, server) = spawn(disk, hurl_kinds());

    init(&client);
    let url = name_to_url(&feature_name).unwrap();
    open(&client, &url, &feature_text());

    // Collect notifications until we see publishDiagnostics for our file.
    let params = wait_for_diagnostics(&client, &url);
    assert!(
        params.diagnostics.iter().any(|d| matches!(
            &d.code, Some(lsp_types::Code::String(c)) if c == "proef::bind::unbound_step")),
        "expected unbound_step, got {:?}",
        params.diagnostics
    );
    // range must be non-degenerate (points at the offending step, not 0:0-0:0)
    let d = &params.diagnostics[0];
    assert!(d.range.end > d.range.start);

    shutdown(&client, server);
}

#[test]
fn definition_on_a_step_jumps_to_the_macro() {
    let feature_name = native_abs("suite/f.feature");
    let pack_name = native_abs("suite/packs/p.yaml");
    let mut files = BTreeMap::new();
    let feature_text = "Feature: F\n  Scenario: S\n    When I greet Sam\n";
    files.insert(feature_name.clone(), Arc::from(feature_text));
    files.insert(
        pack_name.clone(),
        Arc::from(
            "macros:\n  greet:\n    params: [who]\n    match: \"I greet {who}\"\n    steps:\n      - hurl: |\n          GET http://x\n",
        ),
    );
    let disk = FakeDisk {
        features: vec![feature_name.clone()],
        packs: vec![pack_name.clone()],
        fragments: Vec::new(),
        files,
    };

    let (client, server) = spawn(disk, hurl_kinds());
    init(&client);
    let url = name_to_url(&feature_name).unwrap();
    open(&client, &url, feature_text);
    // wait for the initial diagnostics so we know a recompute has happened
    let _ = wait_for_any_diagnostics(&client);

    // "    When I greet Sam" is line 2; char 9 lands inside the step's span
    // (which starts at the "When" keyword), well before the step ends.
    client
        .sender
        .send(Message::Request(Request {
            id: RequestId::from(10),
            method: "textDocument/definition".to_owned(),
            params: serde_json::to_value(DefinitionParams {
                text_document_position_params: TextDocumentPositionParams {
                    text_document: TextDocumentIdentifier { uri: url.clone() },
                    position: Position {
                        line: 2,
                        character: 9,
                    },
                },
                work_done_progress_params: WorkDoneProgressParams::default(),
                partial_result_params: PartialResultParams::default(),
            })
            .unwrap(),
        }))
        .unwrap();

    let loc = wait_for_response::<DefinitionResponse>(&client, &RequestId::from(10));
    let target: Location = match loc {
        DefinitionResponse::Definition(Definition::Location(l)) => l,
        DefinitionResponse::Definition(Definition::LocationList(mut v)) => v.remove(0),
        DefinitionResponse::DefinitionLinkList(links) => {
            panic!("unexpected definition response: {links:?}")
        }
    };
    assert_eq!(target.uri, name_to_url(&pack_name).unwrap());
    // `match: "I greet {who}"` is line 3 of the pack — the landing anchor
    // preferred over the macro's name-key line (line 1).
    assert_eq!(target.range.start.line, 3);

    shutdown(&client, server);
}

#[test]
fn definition_on_a_use_line_jumps_to_the_target_macro() {
    let feature_name = native_abs("suite/f.feature");
    let pack_name = native_abs("suite/packs/p.yaml");
    let pack_text = "macros:\n  base:\n    match: I am the base\n    steps:\n      - hurl: |\n          GET http://x\n  wrapper:\n    match: the wrapper\n    steps:\n      - use: base\n";
    let mut files = BTreeMap::new();
    files.insert(feature_name.clone(), Arc::from(feature_text()));
    files.insert(pack_name.clone(), Arc::from(pack_text));
    let disk = FakeDisk {
        features: vec![feature_name.clone()],
        packs: vec![pack_name.clone()],
        fragments: Vec::new(),
        files,
    };

    let (client, server) = spawn(disk, hurl_kinds());
    init(&client);
    let pack_url = name_to_url(&pack_name).unwrap();
    open(&client, &pack_url, pack_text);
    let _ = wait_for_any_diagnostics(&client);

    // "      - use: base" is line 9; char 13 lands inside `use: base`'s span
    // (on the "base" target name).
    client
        .sender
        .send(Message::Request(Request {
            id: RequestId::from(10),
            method: "textDocument/definition".to_owned(),
            params: serde_json::to_value(DefinitionParams {
                text_document_position_params: TextDocumentPositionParams {
                    text_document: TextDocumentIdentifier {
                        uri: pack_url.clone(),
                    },
                    position: Position {
                        line: 9,
                        character: 13,
                    },
                },
                work_done_progress_params: WorkDoneProgressParams::default(),
                partial_result_params: PartialResultParams::default(),
            })
            .unwrap(),
        }))
        .unwrap();

    let loc = wait_for_response::<DefinitionResponse>(&client, &RequestId::from(10));
    let target: Location = match loc {
        DefinitionResponse::Definition(Definition::Location(l)) => l,
        DefinitionResponse::Definition(Definition::LocationList(mut v)) => v.remove(0),
        DefinitionResponse::DefinitionLinkList(links) => {
            panic!("unexpected definition response: {links:?}")
        }
    };
    assert_eq!(target.uri, pack_url);
    // `base`'s `match:` line is line 2 — the anchor `use:` resolves to.
    assert_eq!(target.range.start.line, 2);

    shutdown(&client, server);
}

/// Go-to-definition from a `ref:` line lands on the annotation in the fragment
/// file — the editor half of ADR-0018, end to end through the real provider
/// chain.
///
/// This exists because that chain broke silently once: `discover_fragments` had
/// a default `Ok(Vec::new())`, and the LSP's overlay provider forwarded the
/// other two discoveries without overriding it. Every `ref:` then read as
/// `unknown_ref` in the editor while the same suite ran green — and no test
/// noticed, because every fake provider inherited the same default. The trait
/// method is now required; this asserts the wiring it forces.
#[test]
fn definition_on_a_ref_line_jumps_into_the_fragment_file() {
    let feature_name = native_abs("suite/f.feature");
    let pack_name = native_abs("suite/packs/p.yaml");
    let fragment_name = native_abs("corpus/api.hurl");
    let pack_text =
        "macros:\n  search:\n    match: the wrapper\n    steps:\n      - ref: task.search\n";
    // Line 0 is a header comment, so the annotation is line 1 — a fragment
    // anchors on its own annotation, never on the file's leading prose.
    let fragment_text = "# corpus header\n@task.search\n?q\n";
    let mut files = BTreeMap::new();
    files.insert(feature_name.clone(), Arc::from(feature_text()));
    files.insert(pack_name.clone(), Arc::from(pack_text));
    files.insert(fragment_name.clone(), Arc::from(fragment_text));
    let disk = FakeDisk {
        features: vec![feature_name.clone()],
        packs: vec![pack_name.clone()],
        fragments: vec![fragment_name.clone()],
        files,
    };

    let (client, server) = spawn(disk, hurl_kinds_with_fragments());
    init(&client);
    let pack_url = name_to_url(&pack_name).unwrap();
    open(&client, &pack_url, pack_text);
    let diags = wait_for_any_diagnostics(&client);

    // The `ref:` resolves, so it must not report `unknown_ref` anywhere.
    assert!(
        !diags
            .diagnostics
            .iter()
            .any(|d| format!("{d:?}").contains("unknown_ref")),
        "a resolvable ref: must not be an error in the editor: {diags:?}"
    );

    // "      - ref: task.search" is line 4; char 15 lands on the target name.
    client
        .sender
        .send(Message::Request(Request {
            id: RequestId::from(10),
            method: "textDocument/definition".to_owned(),
            params: serde_json::to_value(DefinitionParams {
                text_document_position_params: TextDocumentPositionParams {
                    text_document: TextDocumentIdentifier {
                        uri: pack_url.clone(),
                    },
                    position: Position {
                        line: 4,
                        character: 15,
                    },
                },
                work_done_progress_params: WorkDoneProgressParams::default(),
                partial_result_params: PartialResultParams::default(),
            })
            .unwrap(),
        }))
        .unwrap();

    let loc = wait_for_response::<DefinitionResponse>(&client, &RequestId::from(10));
    let target: Location = match loc {
        DefinitionResponse::Definition(Definition::Location(l)) => l,
        DefinitionResponse::Definition(Definition::LocationList(mut v)) => v.remove(0),
        DefinitionResponse::DefinitionLinkList(links) => {
            panic!("unexpected definition response: {links:?}")
        }
    };
    assert_eq!(
        target.uri,
        name_to_url(&fragment_name).unwrap(),
        "the jump leaves the pack and lands in the fragment file"
    );
    assert_eq!(
        target.range.start.line, 1,
        "on the `@task.search` annotation, not the file header"
    );

    shutdown(&client, server);
}

#[test]
fn definition_on_a_step_lands_on_the_match_line() {
    let feature_name = native_abs("suite/f.feature");
    let pack_name = native_abs("suite/packs/p.yaml");
    let feature_text = "Feature: F\n  Scenario: S\n    When the wrapper\n";
    let pack_text = "macros:\n  base:\n    match: I am the base\n    steps:\n      - hurl: |\n          GET http://x\n  wrapper:\n    match: the wrapper\n    steps:\n      - use: base\n";
    let mut files = BTreeMap::new();
    files.insert(feature_name.clone(), Arc::from(feature_text));
    files.insert(pack_name.clone(), Arc::from(pack_text));
    let disk = FakeDisk {
        features: vec![feature_name.clone()],
        packs: vec![pack_name.clone()],
        fragments: Vec::new(),
        files,
    };

    let (client, server) = spawn(disk, hurl_kinds());
    init(&client);
    let url = name_to_url(&feature_name).unwrap();
    open(&client, &url, feature_text);
    let _ = wait_for_any_diagnostics(&client);

    // "    When the wrapper" is line 2; char 9 lands inside the step's span.
    client
        .sender
        .send(Message::Request(Request {
            id: RequestId::from(10),
            method: "textDocument/definition".to_owned(),
            params: serde_json::to_value(DefinitionParams {
                text_document_position_params: TextDocumentPositionParams {
                    text_document: TextDocumentIdentifier { uri: url.clone() },
                    position: Position {
                        line: 2,
                        character: 9,
                    },
                },
                work_done_progress_params: WorkDoneProgressParams::default(),
                partial_result_params: PartialResultParams::default(),
            })
            .unwrap(),
        }))
        .unwrap();

    let loc = wait_for_response::<DefinitionResponse>(&client, &RequestId::from(10));
    let target: Location = match loc {
        DefinitionResponse::Definition(Definition::Location(l)) => l,
        DefinitionResponse::Definition(Definition::LocationList(mut v)) => v.remove(0),
        DefinitionResponse::DefinitionLinkList(links) => {
            panic!("unexpected definition response: {links:?}")
        }
    };
    assert_eq!(target.uri, name_to_url(&pack_name).unwrap());
    // `wrapper`'s `match:` line is line 7, NOT its name-key line (6).
    assert_eq!(target.range.start.line, 7);

    shutdown(&client, server);
}

#[test]
fn completion_offers_macro_pattern_snippets() {
    use lsp_types::{CompletionParams, CompletionResponse, InsertTextFormat};

    let feature_name = native_abs("suite/f.feature");
    let pack_name = native_abs("suite/packs/p.yaml");
    let feature_text = "Feature: F\n  Scenario: S\n    When I gr\n";
    let mut files = BTreeMap::new();
    files.insert(feature_name.clone(), Arc::from(feature_text));
    files.insert(
        pack_name.clone(),
        Arc::from(
            "macros:\n  greet:\n    params: [who]\n    match: \"I greet {who}\"\n    steps:\n      - hurl: |\n          GET http://x\n  saved:\n    match: \"the note is saved\"\n    steps:\n      - hurl: |\n          GET http://x\n",
        ),
    );
    let disk = FakeDisk {
        features: vec![feature_name.clone()],
        packs: vec![pack_name],
        fragments: Vec::new(),
        files,
    };

    let (client, server) = spawn(disk, hurl_kinds());
    init(&client);
    let url = name_to_url(&feature_name).unwrap();
    open(&client, &url, feature_text);
    let _ = wait_for_any_diagnostics(&client);

    // "    When I gr" is line 2; character 13 lands at end of line (cursor at EOL).
    client
        .sender
        .send(Message::Request(Request {
            id: RequestId::from(20),
            method: "textDocument/completion".to_owned(),
            params: serde_json::to_value(CompletionParams {
                text_document_position_params: TextDocumentPositionParams {
                    text_document: TextDocumentIdentifier { uri: url.clone() },
                    position: Position {
                        line: 2,
                        character: 13,
                    },
                },
                work_done_progress_params: WorkDoneProgressParams::default(),
                partial_result_params: PartialResultParams::default(),
                context: None,
            })
            .unwrap(),
        }))
        .unwrap();

    let resp = wait_for_response::<CompletionResponse>(&client, &RequestId::from(20));
    let items = match resp {
        CompletionResponse::CompletionItemList(v) => v,
        CompletionResponse::CompletionList(l) => l.items,
    };
    let greet = items
        .iter()
        .find(|i| i.label.contains("greet") || i.label.contains("I greet"))
        .expect("greet completion offered");
    assert_eq!(greet.insert_text_format, Some(InsertTextFormat::Snippet));
    assert!(
        greet.insert_text.as_ref().unwrap().contains("${1:"),
        "capture becomes a tabstop"
    );

    // The prefix-matching macro sorts ahead of the non-matching one.
    let saved = items
        .iter()
        .find(|i| i.label == "the note is saved")
        .expect("saved completion offered");
    assert!(
        greet.sort_text < saved.sort_text,
        "prefix match must sort first: greet={:?} saved={:?}",
        greet.sort_text,
        saved.sort_text
    );
    // filterText is the pattern's prose skeleton, so the editor narrows on prose.
    assert_eq!(greet.filter_text.as_deref(), Some("I greet "));

    shutdown(&client, server);
}

#[test]
fn references_lists_every_step_bound_to_the_macro() {
    use lsp_types::{ReferenceContext, ReferenceParams};

    let f1 = native_abs("suite/a.feature");
    let f2 = native_abs("suite/b.feature");
    let pack = native_abs("suite/packs/p.yaml");
    let t1 = "Feature: A\n  Scenario: S\n    When I greet Sam\n";
    let t2 = "Feature: B\n  Scenario: T\n    When I greet Mia\n";
    let mut files = BTreeMap::new();
    files.insert(f1.clone(), Arc::from(t1));
    files.insert(f2.clone(), Arc::from(t2));
    files.insert(
        pack.clone(),
        Arc::from(
            "macros:\n  greet:\n    params: [who]\n    match: \"I greet {who}\"\n    steps:\n      - hurl: |\n          GET http://x\n",
        ),
    );
    let disk = FakeDisk {
        features: vec![f1.clone(), f2.clone()],
        packs: vec![pack],
        fragments: Vec::new(),
        files,
    };

    let (client, server) = spawn(disk, hurl_kinds());
    init(&client);
    let url1 = name_to_url(&f1).unwrap();
    open(&client, &url1, t1);
    let _ = wait_for_any_diagnostics(&client);

    // "    When I greet Sam" is line 2; char 9 lands inside the step's span.
    client
        .sender
        .send(Message::Request(Request {
            id: RequestId::from(30),
            method: "textDocument/references".to_owned(),
            params: serde_json::to_value(ReferenceParams {
                text_document_position_params: TextDocumentPositionParams {
                    text_document: TextDocumentIdentifier { uri: url1.clone() },
                    position: Position {
                        line: 2,
                        character: 9,
                    },
                },
                work_done_progress_params: WorkDoneProgressParams::default(),
                partial_result_params: PartialResultParams::default(),
                context: ReferenceContext {
                    include_declaration: false,
                },
            })
            .unwrap(),
        }))
        .unwrap();

    let locs = wait_for_response::<Vec<Location>>(&client, &RequestId::from(30));
    assert_eq!(locs.len(), 2, "both greet steps referenced: {locs:?}");

    shutdown(&client, server);
}

#[test]
fn malformed_request_params_are_rejected_without_killing_the_server() {
    let disk = FakeDisk {
        features: Vec::new(),
        packs: Vec::new(),
        fragments: Vec::new(),
        files: BTreeMap::new(),
    };
    let (client, server) = spawn(disk, Vec::new());
    init(&client);

    // A definition request whose URI has no scheme — `lsp_types::Uri` is
    // `url::Url`, which rejects a relative reference with no base, so params
    // deserialization fails. (A raw space would *not* do: url percent-encodes
    // it and parses happily.)
    client
        .sender
        .send(Message::Request(Request {
            id: RequestId::from(10),
            method: "textDocument/definition".to_owned(),
            params: serde_json::json!({
                "textDocument": { "uri": "not-a-uri" },
                "position": { "line": 0, "character": 0 }
            }),
        }))
        .unwrap();

    // The server must answer with an InvalidParams (-32602) error, not die.
    let resp = wait_for_response_message(&client, &RequestId::from(10));
    let Message::Response(resp) = resp else {
        panic!("expected a response, got {resp:?}");
    };
    let err = resp
        .response_result
        .expect_err("malformed params must produce an error response");
    assert_eq!(err.code, -32602, "expected InvalidParams, got {err:?}");

    // Proof of life: a valid request afterwards is still answered normally.
    client
        .sender
        .send(Message::Request(Request {
            id: RequestId::from(11),
            method: "textDocument/definition".to_owned(),
            params: serde_json::to_value(DefinitionParams {
                text_document_position_params: TextDocumentPositionParams {
                    text_document: TextDocumentIdentifier {
                        uri: name_to_url(&native_abs("suite/none.feature")).unwrap(),
                    },
                    position: Position {
                        line: 0,
                        character: 0,
                    },
                },
                work_done_progress_params: WorkDoneProgressParams::default(),
                partial_result_params: PartialResultParams::default(),
            })
            .unwrap(),
        }))
        .unwrap();
    let alive: Option<DefinitionResponse> = wait_for_response(&client, &RequestId::from(11));
    assert!(
        alive.is_none(),
        "no binding exists, so a null result — but the server answered"
    );

    shutdown(&client, server);
}

/// Two feature requests with no edit between them share one analysis; an edit
/// between them does not.
///
/// Every feature — completion, definition, references — used to run the whole
/// pipeline from scratch: read every pack and feature off the provider, parse,
/// bind, lower, then throw the result away. Between two keystrokes none of its
/// inputs have changed, so the second run could only reproduce the first one's
/// answer. Counting provider reads is how that is pinned without timing
/// anything: reads are the pipeline's first act, so a recompute cannot happen
/// without them, and the count is deterministic where a duration is not.
#[test]
fn an_analysis_is_reused_until_an_edit_retires_it() {
    let feature_name = native_abs("suite/f.feature");
    let pack_name = native_abs("suite/packs/p.yaml");
    let feature_text = "Feature: F\n  Scenario: S\n    When I greet Sam\n";
    let mut files = BTreeMap::new();
    files.insert(feature_name.clone(), Arc::from(feature_text));
    files.insert(
        pack_name.clone(),
        Arc::from(
            "macros:\n  greet:\n    params: [who]\n    match: \"I greet {who}\"\n    steps:\n      - hurl: |\n          GET http://x\n",
        ),
    );
    let reads = Arc::new(AtomicUsize::new(0));
    let disk = CountingDisk {
        inner: FakeDisk {
            features: vec![feature_name.clone()],
            packs: vec![pack_name],
            fragments: Vec::new(),
            files,
        },
        reads: Arc::clone(&reads),
    };

    let (client, server) = spawn(disk, hurl_kinds());
    init(&client);
    let url = name_to_url(&feature_name).unwrap();
    open(&client, &url, feature_text);
    let _ = wait_for_any_diagnostics(&client);

    let definition_at = |id: i32| {
        client
            .sender
            .send(Message::Request(Request {
                id: RequestId::from(id),
                method: "textDocument/definition".to_owned(),
                params: serde_json::to_value(DefinitionParams {
                    text_document_position_params: TextDocumentPositionParams {
                        text_document: TextDocumentIdentifier { uri: url.clone() },
                        position: Position {
                            line: 2,
                            character: 9,
                        },
                    },
                    work_done_progress_params: WorkDoneProgressParams::default(),
                    partial_result_params: PartialResultParams::default(),
                })
                .unwrap(),
            }))
            .unwrap();
        wait_for_response::<Option<DefinitionResponse>>(&client, &RequestId::from(id))
    };

    // The debounced recompute above already filled the cache, so the first
    // request reads nothing at all.
    let before = reads.load(Ordering::SeqCst);
    assert!(definition_at(40).is_some(), "the step resolves");
    let after_first = reads.load(Ordering::SeqCst);
    assert_eq!(
        after_first, before,
        "a request after a recompute reuses that analysis"
    );

    // A second request, still no edit: nothing to reread.
    assert!(definition_at(41).is_some(), "the step still resolves");
    assert_eq!(
        reads.load(Ordering::SeqCst),
        after_first,
        "a second request with no edit between reuses the same analysis"
    );

    // An edit retires it: the next request must see the new text, so it reads.
    client
        .sender
        .send(Message::Notification(Notification {
            method: "textDocument/didChange".to_owned(),
            params: serde_json::json!({
                "textDocument": { "uri": url.as_str(), "version": 2 },
                "contentChanges": [{ "text": feature_text }]
            }),
        }))
        .unwrap();
    let _ = wait_for_any_diagnostics(&client);
    assert!(
        reads.load(Ordering::SeqCst) > after_first,
        "an edit invalidates the cache — the suite is read again"
    );

    shutdown(&client, server);
}

/// A typo'd `use:` target offers the quick fix that corrects it, as a real edit
/// on the real bytes.
///
/// The did-you-mean already existed in the message; what an editor could not do
/// was *apply* it. `Diag::with_fix_replacing` attaches the structured half
/// (span + replacement) only where the edit is certain, and this asserts the
/// whole chain: pack validation finds the typo, the fix survives into the
/// analysis, and the action's `TextEdit` covers exactly the misspelled token.
#[test]
fn a_misspelled_use_target_offers_the_correcting_edit() {
    use lsp_types::{CodeActionKind, CodeActionResponse};

    let (client, server, pack_url) = a_pack_with_a_typo();

    let at_cursor = code_action_probe(&client, &pack_url);
    // "      - use: basse" is line 9; the cursor sits inside `basse` (col 15).
    let actions = at_cursor(50, 9, 15);
    let action = actions
        .iter()
        .find_map(|a| match a {
            CodeActionResponse::CodeAction(a) => Some(a),
            CodeActionResponse::Command(_) => None,
        })
        .expect("the typo offers a quick fix");

    assert_eq!(action.title, "replace `basse` with `base`");
    assert_eq!(action.kind, Some(CodeActionKind::QuickFix));
    assert_eq!(action.is_preferred, Some(true));
    // The action names its diagnostic — which carets the macro's *name key* on
    // line 6, three lines above the edit. That split is the normal case, not an
    // anomaly, and it is why a fix is found in the file rather than in the span
    // and offered from either end.
    let named = action
        .diagnostics
        .as_ref()
        .expect("the action names the diagnostic it fixes");
    assert_eq!(named.len(), 1);
    assert_eq!(named[0].range.start.line, 6, "caret on `wrapper:`");
    assert_eq!(
        named[0].code,
        Some(lsp_types::Code::String(
            "proef::pack::unknown_use".to_owned()
        ))
    );

    let edits = action
        .edit
        .as_ref()
        .and_then(|e| e.changes.as_ref())
        .and_then(|c| c.get(&pack_url))
        .expect("an edit for this very file");
    assert_eq!(edits.len(), 1);
    assert_eq!(edits[0].new_text, "base");
    // The edit covers `basse` and nothing else: line 9, the five columns after
    // "      - use: ". Applying it by hand yields the corrected line, which is
    // the only claim that matters to an author.
    assert_eq!(edits[0].range.start.line, 9);
    let (start, end) = (
        edits[0].range.start.character as usize,
        edits[0].range.end.character as usize,
    );
    assert_eq!((start, end), (13, 18));
    let line = TYPO_PACK.lines().nth(9).unwrap();
    assert_eq!(
        format!("{}{}{}", &line[..start], edits[0].new_text, &line[end..]),
        "      - use: base"
    );

    shutdown(&client, server);
}

/// The same fix is reachable from the squiggle *and* from the token, and from
/// nowhere else.
///
/// The two are three lines apart here — the diagnostic carets `wrapper:`, the
/// typo is in the step below — and an author arrives at one or the other
/// depending on whether they followed the squiggle or just finished typing.
/// Offering at only one end hides the fix from half of them.
#[test]
fn a_quick_fix_is_reachable_from_the_diagnostic_and_from_the_token() {
    let (client, server, pack_url) = a_pack_with_a_typo();
    let at_cursor = code_action_probe(&client, &pack_url);

    assert_eq!(at_cursor(50, 9, 15).len(), 1, "from the token");
    assert_eq!(at_cursor(51, 6, 4).len(), 1, "from the diagnostic");
    // A healthy line offers nothing — a quick fix that appears everywhere is
    // noise, and `base` on line 1 is correct exactly as written.
    let elsewhere = at_cursor(52, 1, 4);
    assert!(
        elsewhere.is_empty(),
        "an unrelated line offers no fix: {elsewhere:?}"
    );

    shutdown(&client, server);
}

/// The suite both quick-fix tests run against: a pack whose `wrapper` macro
/// says `use: basse` while the loaded macro is `base`.
fn a_pack_with_a_typo() -> (Connection, std::thread::JoinHandle<()>, Uri) {
    let feature_name = native_abs("suite/f.feature");
    let pack_name = native_abs("suite/packs/p.yaml");
    let mut files = BTreeMap::new();
    files.insert(feature_name.clone(), Arc::from(feature_text()));
    files.insert(pack_name.clone(), Arc::from(TYPO_PACK));
    let disk = FakeDisk {
        features: vec![feature_name.clone()],
        packs: vec![pack_name.clone()],
        fragments: Vec::new(),
        files,
    };
    let (client, server) = spawn(disk, hurl_kinds());
    init(&client);
    let pack_url = name_to_url(&pack_name).unwrap();
    open(&client, &pack_url, TYPO_PACK);
    let _ = wait_for_any_diagnostics(&client);
    (client, server, pack_url)
}

/// `wrapper` uses `basse`; the loaded macro is `base`.
const TYPO_PACK: &str = "macros:\n  base:\n    match: I am the base\n    steps:\n      - hurl: |\n          GET http://x\n  wrapper:\n    match: the wrapper\n    steps:\n      - use: basse\n";

/// A feature outlines to its scenarios, a pack to its macros — and a hover on
/// a bound step names the macro without leaving the line.
///
/// Both read the same analysis the other features do, so this asserts the two
/// vocabularies land on the right file kinds and that hover resolves through
/// the binding index rather than by re-parsing text.
#[test]
fn a_file_outlines_to_its_own_vocabulary_and_a_step_hovers_to_its_macro() {
    use lsp_types::{
        Contents, DocumentSymbolParams, DocumentSymbolResponse, Hover, HoverParams, SymbolKind,
    };

    let feature_name = native_abs("suite/f.feature");
    let pack_name = native_abs("suite/packs/p.yaml");
    let feature_text = "@smoke\nFeature: F\n  Scenario: greets a person\n    When I greet Sam\n  Scenario: greets nobody\n    When I greet Mia\n";
    let pack_text = "macros:\n  greet:\n    params: [who]\n    match: \"I greet {who}\"\n    steps:\n      - hurl: |\n          GET http://x\n";
    let mut files = BTreeMap::new();
    files.insert(feature_name.clone(), Arc::from(feature_text));
    files.insert(pack_name.clone(), Arc::from(pack_text));
    let disk = FakeDisk {
        features: vec![feature_name.clone()],
        packs: vec![pack_name.clone()],
        fragments: Vec::new(),
        files,
    };

    let (client, server) = spawn(disk, hurl_kinds());
    init(&client);
    let feature_url = name_to_url(&feature_name).unwrap();
    let pack_url = name_to_url(&pack_name).unwrap();
    open(&client, &feature_url, feature_text);
    let _ = wait_for_any_diagnostics(&client);

    let outline = |id: i32, url: &Uri| {
        client
            .sender
            .send(Message::Request(Request {
                id: RequestId::from(id),
                method: "textDocument/documentSymbol".to_owned(),
                params: serde_json::to_value(DocumentSymbolParams {
                    text_document: TextDocumentIdentifier { uri: url.clone() },
                    work_done_progress_params: WorkDoneProgressParams::default(),
                    partial_result_params: PartialResultParams::default(),
                })
                .unwrap(),
            }))
            .unwrap();
        match wait_for_response::<Option<DocumentSymbolResponse>>(&client, &RequestId::from(id)) {
            Some(DocumentSymbolResponse::DocumentSymbolList(v)) => v,
            other => panic!("expected a symbol list, got {other:?}"),
        }
    };

    // A feature outlines to its scenarios, in authored order, on their headers.
    let scenarios = outline(60, &feature_url);
    let names: Vec<&str> = scenarios.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["greets a person", "greets nobody"]);
    assert_eq!(scenarios[0].kind, SymbolKind::Method);
    assert_eq!(scenarios[0].range.start.line, 2, "the `Scenario:` header");
    // Feature-level tags accumulate onto every scenario, and an outline is
    // where `@skip`/`@slow` earn their glance.
    assert_eq!(scenarios[0].detail.as_deref(), Some("@smoke"));

    // A pack outlines to its macros, detailed by the pattern they match.
    let macros = outline(61, &pack_url);
    assert_eq!(macros.len(), 1);
    assert_eq!(macros[0].name, "greet");
    assert_eq!(macros[0].kind, SymbolKind::Function);
    assert_eq!(macros[0].detail.as_deref(), Some("I greet {who}"));

    // Hover on the bound step names the macro, its pack, its pattern and params.
    client
        .sender
        .send(Message::Request(Request {
            id: RequestId::from(62),
            method: "textDocument/hover".to_owned(),
            params: serde_json::to_value(HoverParams {
                text_document_position_params: TextDocumentPositionParams {
                    text_document: TextDocumentIdentifier {
                        uri: feature_url.clone(),
                    },
                    position: Position {
                        line: 3,
                        character: 9,
                    },
                },
                work_done_progress_params: WorkDoneProgressParams::default(),
            })
            .unwrap(),
        }))
        .unwrap();
    let hover: Hover = wait_for_response::<Option<Hover>>(&client, &RequestId::from(62))
        .expect("a bound step hovers");
    let Contents::MarkupContent(markup) = hover.contents else {
        panic!("expected markup contents");
    };
    assert!(markup.value.contains("`greet`"), "{}", markup.value);
    assert!(markup.value.contains("I greet {who}"), "{}", markup.value);
    assert!(markup.value.contains("`who`"), "{}", markup.value);
    // The highlighted range is the step, not a guessed word boundary.
    assert_eq!(hover.range.map(|r| r.start.line), Some(3));

    shutdown(&client, server);
}

fn feature_text() -> String {
    "Feature: E\n  Scenario: S\n    When I serch for Jansen\n".to_owned()
}

/// An absolute path valid on the current OS, built from a `/`-relative tail.
/// Windows needs a drive prefix for `Path::is_absolute` (and thus the
/// `name_to_url` bridge) to accept a path; Unix keeps the plain `/`-rooted form.
fn native_abs(rel: &str) -> String {
    #[cfg(windows)]
    {
        format!("C:\\{}", rel.replace('/', "\\"))
    }
    #[cfg(not(windows))]
    {
        format!("/{rel}")
    }
}

/// A `bind:` table completes against what the fragments this pack refs actually
/// read (ADR-0018).
///
/// The names live in the `.hurl` file, which is exactly the file an author
/// adopting a foreign corpus has not memorised. Without this the only way to
/// learn them is to run the suite and read `lower::unbound_placeholder` — an
/// error that arrives at lower time, i.e. after a failure.
#[test]
fn completion_inside_bind_offers_the_fragments_variables() {
    use lsp_types::{CompletionParams, CompletionResponse};

    let feature_name = native_abs("suite/f.feature");
    let pack_name = native_abs("suite/packs/p.yaml");
    let fragment_name = native_abs("corpus/api.hurl");
    // Two placeholders on the reffed fragment, one on a fragment nothing refs —
    // the unreferenced one must not be offered. `region` is read *and* supplied
    // by the fragment itself, so it needs no `bind:` and must not be offered
    // either: binding it would be refused as `option_declared_twice`.
    let fragment_text = "@task.search\n?q\n?index\n=region\n@task.delete\n?doomed\n";
    let pack_text = "macros:\n  search:\n    match: the wrapper\n    steps:\n      - ref: task.search\n        bind:\n          \n";
    let mut files = BTreeMap::new();
    files.insert(feature_name.clone(), Arc::from(feature_text()));
    files.insert(pack_name.clone(), Arc::from(pack_text));
    files.insert(fragment_name.clone(), Arc::from(fragment_text));
    let disk = FakeDisk {
        features: vec![feature_name.clone()],
        packs: vec![pack_name.clone()],
        fragments: vec![fragment_name],
        files,
    };

    let (client, server) = spawn(disk, hurl_kinds_with_fragments());
    init(&client);
    let pack_url = name_to_url(&pack_name).unwrap();
    open(&client, &pack_url, pack_text);
    let _ = wait_for_any_diagnostics(&client);

    // Line 6 is the indented blank line under `bind:` — its parent is `bind:`,
    // which is what puts the cursor in a bind table.
    client
        .sender
        .send(Message::Request(Request {
            id: RequestId::from(30),
            method: "textDocument/completion".to_owned(),
            params: serde_json::to_value(CompletionParams {
                text_document_position_params: TextDocumentPositionParams {
                    text_document: TextDocumentIdentifier {
                        uri: pack_url.clone(),
                    },
                    position: Position {
                        line: 6,
                        character: 10,
                    },
                },
                work_done_progress_params: WorkDoneProgressParams::default(),
                partial_result_params: PartialResultParams::default(),
                context: None,
            })
            .unwrap(),
        }))
        .unwrap();

    let resp = wait_for_response::<CompletionResponse>(&client, &RequestId::from(30));
    let items = match resp {
        CompletionResponse::CompletionItemList(v) => v,
        CompletionResponse::CompletionList(l) => l.items,
    };
    let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
    assert!(
        labels.contains(&"q") && labels.contains(&"index"),
        "the reffed fragment's reads must be offered: {labels:?}"
    );
    assert!(
        !labels.contains(&"doomed"),
        "a fragment this pack never refs must not be offered: {labels:?}"
    );
    assert!(
        !labels.contains(&"region"),
        "a variable the fragment supplies itself needs no `bind:` — offering it \
         would propose an edit `option_declared_twice` then rejects: {labels:?}"
    );
    let q = items.iter().find(|i| i.label == "q").unwrap();
    assert_eq!(q.detail.as_deref(), Some("read by task.search"));
    assert_eq!(
        q.insert_text.as_deref(),
        Some("q: "),
        "the completion writes the key, ready for its value"
    );

    shutdown(&client, server);
}
