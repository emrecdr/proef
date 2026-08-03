//! The stdio LSP event loop: handshake, dispatch, and the debounced whole-suite
//! recompute driver. Single-threaded — the analysis is milliseconds, so v1
//! needs no worker pool. Edits mark the suite dirty; a short debounce coalesces
//! a burst of keystrokes into one recompute that republishes diagnostics.

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossbeam_channel::RecvTimeoutError;
use lsp_server::{Connection, IoThreads, Message};
use lsp_types::ServerCapabilities;
use lsp_types::notification::{
    DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument, Notification as _,
};
use proef_core::engine::StepKindSpec;
use proef_core::provider::SourceProvider;

use crate::analysis::{Analysis, RecomputeInputs, recompute};
use crate::documents::Documents;
use crate::features::{definition, diagnostics};

/// How the server talks to its client.
pub enum Transport {
    /// Real stdio (production).
    Stdio,
    /// An in-process connection (tests).
    InMemory(Connection),
}

/// Startup configuration for [`run`]. Everything the sans-IO analysis needs is
/// injected here at the process edge: the disk provider, the engine registry,
/// and the resolved variable scopes.
pub struct ServerConfig {
    /// How the server talks to its client.
    pub transport: Transport,
    /// The suite root the disk provider walks.
    pub root: PathBuf,
    /// The disk-backed source provider (open buffers override its bytes).
    pub disk: Box<dyn SourceProvider + Send>,
    /// Registered engine step kinds (drives pack validation).
    pub kinds: Vec<StepKindSpec>,
    /// Step-kind prefix → engine id, the lowering routing table.
    pub kind_to_engine: BTreeMap<String, String>,
    /// Injected environment snapshot (`${env:…}`).
    pub env: BTreeMap<String, String>,
    /// Injected `proef.toml` config scope (`${url:…}` / `${vars:…}`).
    pub config_vars: BTreeMap<String, String>,
    /// Debounce window coalescing a burst of edits; tests set 0 for determinism.
    pub debounce: Duration,
}

/// Failure modes of the LSP event loop.
#[derive(Debug)]
pub enum ServerError {
    /// The client violated the LSP wire protocol, or a request we sent failed.
    Protocol(String),
    /// The underlying transport (stdio threads) failed.
    Io(std::io::Error),
}

impl std::fmt::Display for ServerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ServerError::Protocol(m) => write!(f, "LSP protocol error: {m}"),
            ServerError::Io(e) => write!(f, "LSP transport IO error: {e}"),
        }
    }
}

impl std::error::Error for ServerError {}

/// v1 capabilities. Diagnostics are push-model (server-initiated), so they need
/// no capability flag; go-to-definition is advertised here; completion/references
/// are filled in as their tasks land. Text sync is FULL (we recompute wholesale
/// anyway).
fn capabilities() -> ServerCapabilities {
    use lsp_types::{OneOf, TextDocumentSyncCapability, TextDocumentSyncKind};
    ServerCapabilities {
        text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
        definition_provider: Some(OneOf::Left(true)),
        ..Default::default()
    }
}

/// Runs the LSP server to completion: blocks on the `initialize` handshake,
/// dispatches messages until `shutdown`/`exit`, then joins the transport.
pub fn run(mut cfg: ServerConfig) -> Result<(), ServerError> {
    // Take the transport out so the loop can borrow the rest of `cfg`; the
    // transport is only needed to build the connection, never read again.
    let (connection, io_threads): (Connection, Option<IoThreads>) =
        match std::mem::replace(&mut cfg.transport, Transport::Stdio) {
            Transport::Stdio => {
                let (c, t) = Connection::stdio();
                (c, Some(t))
            }
            Transport::InMemory(c) => (c, None),
        };

    // `ServerCapabilities` is plain data (enums, options, strings) so this
    // cannot fail in practice, but library code never unwraps/expects — an
    // encoding failure here becomes a protocol error like any other.
    let caps = serde_json::to_value(capabilities())
        .map_err(|e| ServerError::Protocol(format!("capabilities serialize: {e}")))?;
    // Blocks until the client's `initialize`; the workspace root and providers
    // are injected via `cfg`, so the returned params are unused in v1.
    let _init_params = connection
        .initialize(caps)
        .map_err(|e| ServerError::Protocol(e.to_string()))?;

    main_loop(&connection, &cfg)?;

    if let Some(threads) = io_threads {
        threads.join().map_err(ServerError::Io)?;
    }
    Ok(())
}

/// Mutable server state that outlives a single message: the open-buffer overlay
/// and the set of source names that currently carry published diagnostics.
struct State {
    docs: Documents,
    published: HashSet<String>,
}

fn main_loop(connection: &Connection, cfg: &ServerConfig) -> Result<(), ServerError> {
    let mut state = State {
        docs: Documents::default(),
        published: HashSet::new(),
    };
    let mut dirty_since: Option<Instant> = None;

    loop {
        // Block until the pending recompute is due, or indefinitely if the suite
        // is clean. A due recompute (elapsed >= debounce) runs immediately.
        let timeout = dirty_since.map(|t| {
            let elapsed = t.elapsed();
            cfg.debounce.checked_sub(elapsed).unwrap_or(Duration::ZERO)
        });

        let msg = match timeout {
            Some(d) if d.is_zero() => {
                run_recompute(connection, cfg, &mut state);
                dirty_since = None;
                continue;
            }
            Some(d) => match connection.receiver.recv_timeout(d) {
                Ok(m) => m,
                Err(RecvTimeoutError::Timeout) => {
                    run_recompute(connection, cfg, &mut state);
                    dirty_since = None;
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => return Ok(()),
            },
            None => match connection.receiver.recv() {
                Ok(m) => m,
                Err(_) => return Ok(()),
            },
        };

        match msg {
            Message::Request(req) => {
                if connection
                    .handle_shutdown(&req)
                    .map_err(|e| ServerError::Protocol(e.to_string()))?
                {
                    return Ok(());
                }
                dispatch_request(connection, cfg, &state, &req)?;
            }
            Message::Notification(note) => {
                if apply_notification(&mut state, &note) {
                    dirty_since.get_or_insert_with(Instant::now);
                }
            }
            Message::Response(_) => {}
        }
    }
}

/// Applies a document notification to the overlay. Returns true if it dirtied the
/// suite (an open/change/close that a recompute must react to).
fn apply_notification(state: &mut State, note: &lsp_server::Notification) -> bool {
    match note.method.as_str() {
        DidOpenTextDocument::METHOD => {
            if let Ok(p) =
                serde_json::from_value::<lsp_types::DidOpenTextDocumentParams>(note.params.clone())
            {
                state.docs.open(p.text_document.uri, p.text_document.text);
                return true;
            }
        }
        DidChangeTextDocument::METHOD => {
            if let Ok(p) = serde_json::from_value::<lsp_types::DidChangeTextDocumentParams>(
                note.params.clone(),
            ) {
                // FULL sync → the last change carries the whole document.
                if let Some(change) = p.content_changes.into_iter().last() {
                    state.docs.change(p.text_document.uri, change.text);
                    return true;
                }
            }
        }
        DidCloseTextDocument::METHOD => {
            if let Ok(p) =
                serde_json::from_value::<lsp_types::DidCloseTextDocumentParams>(note.params.clone())
            {
                state.docs.close(&p.text_document.uri);
                return true;
            }
        }
        _ => {}
    }
    false
}

/// Builds the current [`Analysis`] from the live overlay-then-disk provider.
/// The one recompute path: the debounced diagnostics publisher and an
/// on-demand feature request (definition/completion/references) both call
/// this — v1 does not cache the analysis between requests, and the pipeline
/// is milliseconds, so recomputing per request is cheap.
fn current_analysis(cfg: &ServerConfig, state: &State) -> Analysis {
    let inputs = RecomputeInputs {
        root: &cfg.root,
        docs: &state.docs,
        disk: cfg.disk.as_ref(),
        kinds: &cfg.kinds,
        kind_to_engine: &cfg.kind_to_engine,
        env: &cfg.env,
        config_vars: &cfg.config_vars,
    };
    recompute(&inputs)
}

fn run_recompute(connection: &Connection, cfg: &ServerConfig, state: &mut State) {
    // A panic inside analysis must never take the server down: catch it, keep
    // the previously published diagnostics, and let the next edit retry.
    let Ok(analysis) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        current_analysis(cfg, state)
    })) else {
        eprintln!("proef-lsp: suite analysis panicked; keeping previous diagnostics");
        return;
    };
    diagnostics::publish(connection, &analysis, &mut state.published);
}

fn dispatch_request(
    connection: &Connection,
    cfg: &ServerConfig,
    state: &State,
    req: &lsp_server::Request,
) -> Result<(), ServerError> {
    use lsp_types::request::{GotoDefinition, Request as _};

    if req.method == GotoDefinition::METHOD {
        let params: lsp_types::GotoDefinitionParams = serde_json::from_value(req.params.clone())
            .map_err(|e| ServerError::Protocol(e.to_string()))?;
        let analysis = current_analysis(cfg, state);
        let result = definition::goto(
            &analysis,
            &params.text_document_position_params.text_document.uri,
            params.text_document_position_params.position,
        )
        .map(lsp_types::GotoDefinitionResponse::Scalar);
        let resp = lsp_server::Response::new_ok(req.id.clone(), result);
        return connection
            .sender
            .send(Message::Response(resp))
            .map_err(|e| ServerError::Protocol(e.to_string()));
    }

    // Feature requests beyond definition (completion/references) are wired in
    // their own tasks; unknown methods get a method-not-found response so the
    // client never hangs waiting on a reply.
    let resp = lsp_server::Response::new_err(
        req.id.clone(),
        lsp_server::ErrorCode::MethodNotFound as i32,
        format!("unhandled request: {}", req.method),
    );
    connection
        .sender
        .send(Message::Response(resp))
        .map_err(|e| ServerError::Protocol(e.to_string()))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use lsp_server::{Connection, Message, Notification, Request, RequestId};
    use lsp_types::{InitializeParams, InitializedParams};
    use proef_core::provider::ProviderError;
    use std::sync::Arc;

    /// A disk provider with nothing in it — the handshake test opens no
    /// documents, so no recompute ever reads from it.
    struct EmptyDisk;
    impl SourceProvider for EmptyDisk {
        fn discover_features(&self) -> Result<Vec<String>, ProviderError> {
            Ok(Vec::new())
        }
        fn discover_packs(&self) -> Result<Vec<String>, ProviderError> {
            Ok(Vec::new())
        }
        fn read(&self, name: &str) -> Result<Arc<str>, ProviderError> {
            Err(ProviderError(format!("no {name}")))
        }
    }

    #[test]
    fn initialize_then_shutdown_completes_cleanly() {
        // `Connection::memory()` gives us both ends in-process; we drive the
        // client end, the server logic runs on the other.
        let (server_conn, client) = Connection::memory();

        let server = std::thread::spawn(move || {
            run(ServerConfig {
                transport: Transport::InMemory(server_conn),
                root: PathBuf::from("/"),
                disk: Box::new(EmptyDisk),
                kinds: Vec::new(),
                kind_to_engine: BTreeMap::new(),
                env: BTreeMap::new(),
                config_vars: BTreeMap::new(),
                debounce: Duration::ZERO,
            })
            .unwrap();
        });

        // initialize request
        let init = serde_json::to_value(InitializeParams::default()).unwrap();
        client
            .sender
            .send(Message::Request(Request {
                id: RequestId::from(1),
                method: "initialize".to_owned(),
                params: init,
            }))
            .unwrap();
        // expect an InitializeResult response
        let resp = client.receiver.recv().unwrap();
        assert!(
            matches!(resp, Message::Response(_)),
            "expected initialize response, got {resp:?}"
        );

        // initialized notification
        client
            .sender
            .send(Message::Notification(Notification {
                method: "initialized".to_owned(),
                params: serde_json::to_value(InitializedParams {}).unwrap(),
            }))
            .unwrap();

        // shutdown request → expect a null response
        client
            .sender
            .send(Message::Request(Request {
                id: RequestId::from(2),
                method: "shutdown".to_owned(),
                params: serde_json::Value::Null,
            }))
            .unwrap();
        let resp = client.receiver.recv().unwrap();
        assert!(
            matches!(resp, Message::Response(_)),
            "expected shutdown response"
        );

        // exit notification → server loop returns
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
}
