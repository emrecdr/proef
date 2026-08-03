//! Scripted JSON-RPC integration test: drive the server over an in-memory
//! connection with a fake disk provider seeded from the `bind__unbound_step`
//! corpus shape, and assert the published diagnostic code plus a non-degenerate
//! range. No real filesystem and no real engine — a `hurl` step kind with no
//! `validate` probe is enough to load the pack and bind the feature.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use lsp_server::{Connection, Message, Notification, Request, RequestId};
use lsp_types::{
    DidOpenTextDocumentParams, InitializeParams, InitializedParams, PublishDiagnosticsParams,
    TextDocumentItem, Uri,
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

#[test]
fn open_unbound_step_publishes_the_expected_diagnostic() {
    let feature_name = "/suite/case.feature".to_owned();
    let pack_name = "/suite/packs/broken.yaml".to_owned();
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
    client
        .sender
        .send(Message::Notification(Notification {
            method: "textDocument/didOpen".to_owned(),
            params: serde_json::to_value(DidOpenTextDocumentParams {
                text_document: TextDocumentItem {
                    uri: url.clone(),
                    language_id: "gherkin".to_owned(),
                    version: 1,
                    text: feature_text(),
                },
            })
            .unwrap(),
        }))
        .unwrap();

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
    assert!(d.range.end > d.range.start || d.range.start.line > 0);

    // shutdown
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
    drop(client);
    server.join().unwrap();
}

fn feature_text() -> String {
    "Feature: E\n  Scenario: S\n    When I serch for Jansen\n".to_owned()
}

fn wait_for_diagnostics(client: &Connection, url: &Uri) -> PublishDiagnosticsParams {
    loop {
        let msg = client
            .receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("diagnostics timely");
        if let Message::Notification(n) = msg
            && n.method == "textDocument/publishDiagnostics"
        {
            let p: PublishDiagnosticsParams = serde_json::from_value(n.params).unwrap();
            if &p.uri == url && !p.diagnostics.is_empty() {
                return p;
            }
        }
    }
}
