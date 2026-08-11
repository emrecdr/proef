//! The stdio LSP event loop: handshake, dispatch, and the debounced whole-suite
//! recompute driver. Single-threaded — the analysis is milliseconds, so v1
//! needs no worker pool. Edits mark the suite dirty; a short debounce coalesces
//! a burst of keystrokes into one recompute that republishes diagnostics.

use std::collections::{BTreeMap, HashSet};
use std::io::Write as _;
use std::path::{Path, PathBuf};
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
use crate::features::{completion, definition, diagnostics, references};

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
    /// Re-resolve `root`/`disk` once the client announces its workspace.
    ///
    /// The root is decided at the process edge, before this server runs — but
    /// the client only says where the workspace is during `initialize`, which
    /// happens *inside* [`run`]. Without a way back out, the handshake's
    /// `workspaceFolders`/`rootUri` could only be ignored, so a server launched
    /// from `$HOME` analysed `$HOME`. This is that way back: given the client's
    /// root, the caller returns the suite root and a provider over it. Keeping
    /// it a callback is what lets `proef-lsp` stay ignorant of `proef.toml`,
    /// which the CLI owns (ADR-0012).
    ///
    /// `None` (and a client that announces nothing) leaves the injected root
    /// untouched.
    pub resolve_root: Option<RootResolver>,
}

/// See [`ServerConfig::resolve_root`]. `FnOnce`: the handshake happens once.
pub type RootResolver = Box<dyn FnOnce(&Path) -> (PathBuf, Box<dyn SourceProvider + Send>) + Send>;

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
/// no capability flag; go-to-definition, completion, and references are all
/// advertised here. Text sync is FULL (we recompute wholesale anyway).
fn capabilities() -> ServerCapabilities {
    use lsp_types::{CompletionOptions, OneOf, TextDocumentSyncCapability, TextDocumentSyncKind};
    ServerCapabilities {
        text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
        definition_provider: Some(OneOf::Left(true)),
        completion_provider: Some(CompletionOptions {
            trigger_characters: None,
            ..Default::default()
        }),
        references_provider: Some(OneOf::Left(true)),
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
    // Blocks until the client's `initialize`. The root injected via `cfg` is a
    // best guess made from the process working directory; the client is the
    // authority on where its workspace is, so adopt what it says.
    let init_params = connection
        .initialize(caps)
        .map_err(|e| ServerError::Protocol(e.to_string()))?;
    if let Some(client_root) = client_root(&init_params)
        && let Some(resolve) = cfg.resolve_root.take()
    {
        let (root, disk) = resolve(&client_root);
        cfg.root = root;
        cfg.disk = disk;
    }

    main_loop(&connection, &cfg)?;

    // Release the sole writer Sender: lsp-server's stdio writer thread loops on
    // its receiver and only ends when every Sender drops. IoThreads::join()
    // waits on that thread, so joining while `connection` is alive would block
    // forever. Dropping it here lets the writer finish and the join complete.
    drop(connection);

    if let Some(threads) = io_threads {
        threads.join().map_err(ServerError::Io)?;
    }
    Ok(())
}

/// The workspace root the client announced, as a path.
///
/// `workspaceFolders` wins over `rootUri`: `rootUri` has been deprecated since
/// LSP 3.16, and the spec is explicit that a server must ignore it when folders
/// are present. Only the first folder is used — proef analyses one suite, and
/// picking arbitrarily among several would be a worse answer than the
/// configured default.
fn client_root(params: &serde_json::Value) -> Option<PathBuf> {
    let uri = params
        .get("workspaceFolders")
        .and_then(|folders| folders.as_array())
        .and_then(|folders| folders.first())
        .and_then(|folder| folder.get("uri"))
        .or_else(|| params.get("rootUri"))?
        .as_str()?;
    let uri: lsp_types::Uri = uri.parse().ok()?;
    Some(PathBuf::from(crate::documents::url_to_name(&uri)))
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

/// Builds the current [`Analysis`] from the live overlay-then-disk provider, or
/// `None` if analysis panicked. The one recompute path: the debounced
/// diagnostics publisher and every on-demand feature request
/// (definition/completion/references) all call this — v1 does not cache the
/// analysis between requests, and the pipeline is milliseconds, so
/// recomputing per request is cheap. The `catch_unwind` guard lives here, at
/// the one recompute call site, so every caller gets panic safety for free
/// instead of re-wrapping it (or forgetting to).
fn current_analysis(cfg: &ServerConfig, state: &State) -> Option<Analysis> {
    let inputs = RecomputeInputs {
        root: &cfg.root,
        docs: &state.docs,
        disk: cfg.disk.as_ref(),
        kinds: &cfg.kinds,
        kind_to_engine: &cfg.kind_to_engine,
        env: &cfg.env,
        config_vars: &cfg.config_vars,
    };
    // A panic inside analysis must never take the server down: catch it, tell
    // the caller there is nothing new, and let the next edit retry.
    if let Ok(analysis) =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| recompute(&inputs)))
    {
        return Some(analysis);
    }
    // The recovery notice must not become the failure it reports: the
    // `eprintln` macro panics when its write fails, and a closed stderr is EPIPE
    // (Rust ignores SIGPIPE) — which would take down the very server this
    // `catch_unwind` exists to keep alive. Swallow the write error; stderr is
    // the only channel here, so there is nowhere left to report it.
    let _ = writeln!(
        std::io::stderr(),
        "proef-lsp: suite analysis panicked; keeping previous state"
    );
    None
}

fn run_recompute(connection: &Connection, cfg: &ServerConfig, state: &mut State) {
    let Some(analysis) = current_analysis(cfg, state) else {
        return;
    };
    diagnostics::publish(connection, &analysis, &mut state.published);
}

/// Deserialize a request's params, or produce an `InvalidParams` error Response
/// to send back. A malformed request is then *answered* and the loop continues,
/// instead of propagating out of `main_loop` and taking the server down.
// The Err variant carries the Response we're about to send anyway (the rare,
// non-hot malformed-params path), so boxing it would only add an allocation.
#[allow(clippy::result_large_err)]
fn parse_params<P: serde::de::DeserializeOwned>(
    req: &lsp_server::Request,
) -> Result<P, lsp_server::Response> {
    serde_json::from_value(req.params.clone()).map_err(|e| {
        lsp_server::Response::new_err(
            req.id.clone(),
            lsp_server::ErrorCode::InvalidParams as i32,
            format!("invalid params for {}: {e}", req.method),
        )
    })
}

fn dispatch_request(
    connection: &Connection,
    cfg: &ServerConfig,
    state: &State,
    req: &lsp_server::Request,
) -> Result<(), ServerError> {
    use lsp_types::request::{Completion, GotoDefinition, References, Request as _};

    if req.method == GotoDefinition::METHOD {
        let params: lsp_types::GotoDefinitionParams = match parse_params(req) {
            Ok(p) => p,
            Err(resp) => {
                return connection
                    .sender
                    .send(Message::Response(resp))
                    .map_err(|e| ServerError::Protocol(e.to_string()));
            }
        };
        // No analysis (a panicked recompute) is not an error to the client —
        // it just means we have nothing to offer this request; respond with
        // no result rather than propagating the failure.
        let result = current_analysis(cfg, state)
            .and_then(|analysis| {
                definition::goto(
                    &analysis,
                    &params.text_document_position_params.text_document.uri,
                    params.text_document_position_params.position,
                )
            })
            .map(lsp_types::GotoDefinitionResponse::Scalar);
        let resp = lsp_server::Response::new_ok(req.id.clone(), result);
        return connection
            .sender
            .send(Message::Response(resp))
            .map_err(|e| ServerError::Protocol(e.to_string()));
    }

    if req.method == Completion::METHOD {
        let params: lsp_types::CompletionParams = match parse_params(req) {
            Ok(p) => p,
            Err(resp) => {
                return connection
                    .sender
                    .send(Message::Response(resp))
                    .map_err(|e| ServerError::Protocol(e.to_string()));
            }
        };
        // No analysis (a panicked recompute) yields no completions, never a
        // dropped/errored response.
        let items = current_analysis(cfg, state)
            .map(|analysis| {
                completion::complete(
                    &analysis,
                    &params.text_document_position.text_document.uri,
                    params.text_document_position.position,
                )
            })
            .unwrap_or_default();
        let result = Some(lsp_types::CompletionResponse::Array(items));
        let resp = lsp_server::Response::new_ok(req.id.clone(), result);
        return connection
            .sender
            .send(Message::Response(resp))
            .map_err(|e| ServerError::Protocol(e.to_string()));
    }

    if req.method == References::METHOD {
        let params: lsp_types::ReferenceParams = match parse_params(req) {
            Ok(p) => p,
            Err(resp) => {
                return connection
                    .sender
                    .send(Message::Response(resp))
                    .map_err(|e| ServerError::Protocol(e.to_string()));
            }
        };
        // No analysis (a panicked recompute) yields no references, never a
        // dropped/errored response — mirrors definition/completion.
        let locations = current_analysis(cfg, state)
            .map(|analysis| {
                references::find(
                    &analysis,
                    &params.text_document_position.text_document.uri,
                    params.text_document_position.position,
                )
            })
            .unwrap_or_default();
        let resp = lsp_server::Response::new_ok(req.id.clone(), Some(locations));
        return connection
            .sender
            .send(Message::Response(resp))
            .map_err(|e| ServerError::Protocol(e.to_string()));
    }

    // Unknown methods get a method-not-found response so the client never
    // hangs waiting on a reply.
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
                resolve_root: None,
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
