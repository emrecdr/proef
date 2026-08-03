//! Scripted JSON-RPC integration tests: drive the server over an in-memory
//! connection with a fake disk provider, and assert the published diagnostics
//! and go-to-definition responses. No real filesystem and no real engine — a
//! `hurl` step kind with no `validate` probe is enough to load the pack and
//! bind the feature.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use lsp_server::{Connection, Message, Notification, Request, RequestId};
use lsp_types::{
    DidOpenTextDocumentParams, GotoDefinitionParams, GotoDefinitionResponse, InitializeParams,
    InitializedParams, Location, PartialResultParams, Position, PublishDiagnosticsParams,
    TextDocumentIdentifier, TextDocumentItem, TextDocumentPositionParams, Uri,
    WorkDoneProgressParams,
};
use proef_core::engine::StepKindSpec;
use proef_core::provider::{ProviderError, SourceProvider};
use proef_lsp::documents::name_to_url;
use proef_lsp::{ServerConfig, Transport, run};

/// A disk provider seeded from an in-memory map — no real filesystem.
struct FakeDisk {
    features: Vec<String>,
    packs: Vec<String>,
    files: BTreeMap<String, Arc<str>>,
}
impl SourceProvider for FakeDisk {
    fn discover_features(&self) -> Result<Vec<String>, ProviderError> {
        Ok(self.features.clone())
    }
    fn discover_packs(&self) -> Result<Vec<String>, ProviderError> {
        Ok(self.packs.clone())
    }
    fn read(&self, name: &str) -> Result<Arc<str>, ProviderError> {
        self.files
            .get(name)
            .cloned()
            .ok_or_else(|| ProviderError(format!("no {name}")))
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
                    language_id: "gherkin".to_owned(),
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
            return serde_json::from_value(resp.result.unwrap()).unwrap();
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
        files,
    };

    // The `hurl` step kind must be registered (with no `validate` probe) so the
    // pack loads and the feature is bound; an empty registry would fail pack
    // load and short-circuit `analyze_suite` before it reaches the feature.
    let kinds = vec![StepKindSpec {
        prefix: "hurl",
        schema: "true",
        validate: None,
    }];
    let kind_to_engine = BTreeMap::from([("hurl".to_owned(), "hurl".to_owned())]);

    let (server_conn, client) = Connection::memory();
    let server = std::thread::spawn(move || {
        run(ServerConfig {
            transport: Transport::InMemory(server_conn),
            root: PathBuf::from("/suite"),
            disk: Box::new(disk),
            kinds,
            kind_to_engine,
            env: BTreeMap::new(),
            config_vars: BTreeMap::new(),
            debounce: Duration::ZERO,
        })
        .unwrap();
    });

    init(&client);
    let url = name_to_url(&feature_name).unwrap();
    open(&client, &url, &feature_text());

    // Collect notifications until we see publishDiagnostics for our file.
    let params = wait_for_diagnostics(&client, &url);
    assert!(
        params.diagnostics.iter().any(|d| matches!(
            &d.code, Some(lsp_types::NumberOrString::String(c)) if c == "proef::bind::unbound_step")),
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
        files,
    };

    let kinds = vec![StepKindSpec {
        prefix: "hurl",
        schema: "true",
        validate: None,
    }];
    let kind_to_engine = BTreeMap::from([("hurl".to_owned(), "hurl".to_owned())]);

    let (server_conn, client) = Connection::memory();
    let server = std::thread::spawn(move || {
        run(ServerConfig {
            transport: Transport::InMemory(server_conn),
            root: PathBuf::from("/suite"),
            disk: Box::new(disk),
            kinds,
            kind_to_engine,
            env: BTreeMap::new(),
            config_vars: BTreeMap::new(),
            debounce: Duration::ZERO,
        })
        .unwrap();
    });
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
            params: serde_json::to_value(GotoDefinitionParams {
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

    let loc = wait_for_response::<GotoDefinitionResponse>(&client, &RequestId::from(10));
    let target: Location = match loc {
        GotoDefinitionResponse::Scalar(l) => l,
        GotoDefinitionResponse::Array(mut v) => v.remove(0),
        GotoDefinitionResponse::Link(links) => {
            panic!("unexpected definition response: {links:?}")
        }
    };
    assert_eq!(target.uri, name_to_url(&pack_name).unwrap());
    // The macro name "greet" is on line 1 of the pack.
    assert_eq!(target.range.start.line, 1);

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
        files,
    };

    let kinds = vec![StepKindSpec {
        prefix: "hurl",
        schema: "true",
        validate: None,
    }];
    let kind_to_engine = BTreeMap::from([("hurl".to_owned(), "hurl".to_owned())]);

    let (server_conn, client) = Connection::memory();
    let server = std::thread::spawn(move || {
        run(ServerConfig {
            transport: Transport::InMemory(server_conn),
            root: PathBuf::from("/suite"),
            disk: Box::new(disk),
            kinds,
            kind_to_engine,
            env: BTreeMap::new(),
            config_vars: BTreeMap::new(),
            debounce: Duration::ZERO,
        })
        .unwrap();
    });
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
                text_document_position: TextDocumentPositionParams {
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
        CompletionResponse::Array(v) => v,
        CompletionResponse::List(l) => l.items,
    };
    let greet = items
        .iter()
        .find(|i| i.label.contains("greet") || i.label.contains("I greet"))
        .expect("greet completion offered");
    assert_eq!(greet.insert_text_format, Some(InsertTextFormat::SNIPPET));
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
        files,
    };

    let kinds = vec![StepKindSpec {
        prefix: "hurl",
        schema: "true",
        validate: None,
    }];
    let kind_to_engine = BTreeMap::from([("hurl".to_owned(), "hurl".to_owned())]);

    let (server_conn, client) = Connection::memory();
    let server = std::thread::spawn(move || {
        run(ServerConfig {
            transport: Transport::InMemory(server_conn),
            root: PathBuf::from("/suite"),
            disk: Box::new(disk),
            kinds,
            kind_to_engine,
            env: BTreeMap::new(),
            config_vars: BTreeMap::new(),
            debounce: Duration::ZERO,
        })
        .unwrap();
    });
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
                text_document_position: TextDocumentPositionParams {
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
