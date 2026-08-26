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
use lsp_types::{LspNotificationMethod, ServerCapabilities};
use proef_core::engine::StepKindSpec;
use proef_core::pack::FragmentCorpus;
use proef_core::provider::SourceProvider;

use crate::analysis::{Analysis, RecomputeInputs, read_fragments, recompute};
use crate::documents::Documents;
use crate::features::{code_action, completion, definition, diagnostics, references};

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
    use lsp_types::{
        CodeActionKind, CodeActionOptions, CodeActionProvider, CompletionOptions,
        DefinitionProvider, ReferencesProvider, TextDocumentSync, TextDocumentSyncKind,
    };
    ServerCapabilities {
        text_document_sync: Some(TextDocumentSync::Kind(TextDocumentSyncKind::Full)),
        definition_provider: Some(DefinitionProvider::Bool(true)),
        completion_provider: Some(CompletionOptions::default()),
        references_provider: Some(ReferencesProvider::Bool(true)),
        // Announcing the kind, not just `true`: a client that filters by kind
        // (VS Code's "quick fix" keybinding does) skips a server that only
        // says "yes, actions" without saying which.
        code_action_provider: Some(CodeActionProvider::CodeActionOptions(CodeActionOptions {
            code_action_kinds: Some(vec![CodeActionKind::QuickFix]),
            ..CodeActionOptions::default()
        })),
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
    /// The fragment corpus, rebuilt only when a fragment file changes.
    ///
    /// Held here rather than built per recompute: a fresh corpus carries a fresh
    /// scan memo, so building one per request re-read and re-hurl-parsed the
    /// whole corpus on every completion, definition and debounce tick.
    fragments: FragmentCorpus,
    /// A fragment file changed; the corpus is re-read at the next recompute.
    ///
    /// A flag rather than an immediate rebuild, because a rebuild walks the
    /// whole fragment root and re-reads every file in it. Doing that inline in
    /// the notification handler put a full directory walk on the message loop
    /// *per keystroke* while editing a `.hurl` file, delaying the next request's
    /// dispatch and discarding all but the last result. The suite recompute
    /// three lines below has always been debounced for exactly this reason;
    /// the corpus now follows the same policy.
    fragments_dirty: bool,
    /// The last whole-suite analysis, reused until an edit invalidates it.
    ///
    /// Recompute is *the* cost of every feature: completion, definition and
    /// references each ran the full pipeline — read every pack and feature,
    /// parse, bind, lower — and threw the result away. Between two keystrokes
    /// nothing it reads has changed, so the second run could only produce the
    /// first one's answer. `None` means "an edit landed since"; the next
    /// caller rebuilds and refills this.
    ///
    /// It retains the suite's text for the session, which is what the fragment
    /// corpus above already does and what the whole-suite model implies.
    analysis: Option<Analysis>,
}

fn main_loop(connection: &Connection, cfg: &ServerConfig) -> Result<(), ServerError> {
    let mut state = State {
        docs: Documents::default(),
        published: HashSet::new(),
        fragments: read_fragments(&Documents::default(), cfg.disk.as_ref(), &cfg.kinds),
        fragments_dirty: false,
        analysis: None,
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
                dispatch_request(connection, cfg, &mut state, &req)?;
            }
            Message::Notification(note) => {
                if apply_notification(cfg, &mut state, &note) {
                    // The overlay's bytes changed, so every cached answer was
                    // computed from text that no longer exists. Dropped here,
                    // at the one place that knows an edit landed, rather than
                    // at each of the four places that read it.
                    state.analysis = None;
                    dirty_since.get_or_insert_with(Instant::now);
                }
            }
            Message::Response(_) => {}
        }
    }
}

/// Applies a document notification to the overlay. Returns true if it dirtied the
/// suite (an open/change/close that a recompute must react to).
fn apply_notification(
    cfg: &ServerConfig,
    state: &mut State,
    note: &lsp_server::Notification,
) -> bool {
    let touched: lsp_types::Uri = match LspNotificationMethod::from(note.method.as_str()) {
        LspNotificationMethod::TextDocumentDidOpen => {
            let Ok(p) =
                serde_json::from_value::<lsp_types::DidOpenTextDocumentParams>(note.params.clone())
            else {
                return false;
            };
            let uri = p.text_document.uri.clone();
            state.docs.open(p.text_document.uri, p.text_document.text);
            uri
        }
        LspNotificationMethod::TextDocumentDidChange => {
            let Ok(p) = serde_json::from_value::<lsp_types::DidChangeTextDocumentParams>(
                note.params.clone(),
            ) else {
                return false;
            };
            // FULL sync → the last change carries the whole document.
            let Some(change) = p.content_changes.into_iter().last() else {
                return false;
            };
            let text = match change {
                lsp_types::TextDocumentContentChangeEvent::TextDocumentContentChangeWholeDocument(
                    whole,
                ) => whole.text,
                lsp_types::TextDocumentContentChangeEvent::TextDocumentContentChangePartial(
                    partial,
                ) => partial.text,
            };
            let uri = p.text_document.text_document_identifier.uri;
            state.docs.change(uri.clone(), text);
            uri
        }
        LspNotificationMethod::TextDocumentDidClose => {
            let Ok(p) = serde_json::from_value::<lsp_types::DidCloseTextDocumentParams>(
                note.params.clone(),
            ) else {
                return false;
            };
            state.docs.close(&p.text_document.uri);
            p.text_document.uri
        }
        _ => return false,
    };
    // Only a fragment file invalidates the corpus, and only then is it re-read.
    // Editing a pack or a feature — the overwhelming majority of keystrokes —
    // leaves it alone, which is the whole point of holding it across recomputes.
    // A close counts too: the overlay's bytes stop winning, so the corpus must
    // fall back to what is on disk.
    if is_fragment(cfg, &touched) {
        state.fragments_dirty = true;
    }
    true
}

/// Does this document belong to the fragment corpus?
///
/// By extension, asked of the registry rather than hardcoded: a second engine's
/// fragment format must invalidate the corpus exactly as `.hurl` does (ADR-0002).
fn is_fragment(cfg: &ServerConfig, uri: &lsp_types::Uri) -> bool {
    let name = crate::documents::url_to_name(uri);
    cfg.kinds
        .iter()
        .filter_map(|kind| kind.fragments)
        .any(|support| support.claims(&name))
}

/// The current [`Analysis`], from the cache when an edit has not invalidated it
/// and otherwise recomputed into the cache. `None` only if analysis panicked.
///
/// The one read path: the debounced diagnostics publisher and every on-demand
/// feature request (definition/completion/references) all come through here, so
/// they share one recompute per edit rather than one each.
fn analysis<'a>(cfg: &ServerConfig, state: &'a mut State) -> Option<&'a Analysis> {
    if state.analysis.is_none() {
        state.analysis = compute_analysis(cfg, state);
    }
    state.analysis.as_ref()
}

/// Builds an [`Analysis`] from the live overlay-then-disk provider, or `None` if
/// analysis panicked. The `catch_unwind` guard lives here, at the one recompute
/// call site, so every caller gets panic safety for free instead of re-wrapping
/// it (or forgetting to).
fn compute_analysis(cfg: &ServerConfig, state: &State) -> Option<Analysis> {
    let inputs = RecomputeInputs {
        root: &cfg.root,
        docs: &state.docs,
        fragments: &state.fragments,
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
    // One rebuild per debounce window, however many fragment edits it coalesced.
    if std::mem::take(&mut state.fragments_dirty) {
        state.fragments = read_fragments(&state.docs, cfg.disk.as_ref(), &cfg.kinds);
        // The corpus is an analysis input, so a rebuilt one retires whatever a
        // request computed against the old one earlier in this same window.
        state.analysis = None;
    }
    if analysis(cfg, state).is_none() {
        return;
    }
    // Split the borrow: publishing reads the analysis and writes `published`,
    // two disjoint fields of one `&mut State`. The refill above is what makes
    // the `Some` certain; the `else` is the pattern's cost, not a real path.
    let State {
        analysis: Some(current),
        published,
        ..
    } = state
    else {
        return;
    };
    diagnostics::publish(connection, current, published);
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

/// Answer one request: deserialize its params, hand them to `handle` together
/// with the current analysis, and send whatever it returns.
///
/// Every feature shares this shape, including the two ways a request can come
/// to nothing. Malformed params are *answered* with `InvalidParams` rather than
/// propagated, so one bad request never takes the server down. And a `None`
/// analysis — a recompute that panicked — is not an error to the client either:
/// it means we have nothing to offer *this* request, so `handle` decides what
/// empty looks like for its own feature and the client gets a normal response.
fn answer<P, R>(
    connection: &Connection,
    cfg: &ServerConfig,
    state: &mut State,
    req: &lsp_server::Request,
    handle: impl FnOnce(Option<&Analysis>, P) -> R,
) -> Result<(), ServerError>
where
    P: serde::de::DeserializeOwned,
    R: serde::Serialize,
{
    let resp = match parse_params::<P>(req) {
        Ok(params) => {
            lsp_server::Response::new_ok(req.id.clone(), handle(analysis(cfg, state), params))
        }
        Err(invalid) => invalid,
    };
    connection
        .sender
        .send(Message::Response(resp))
        .map_err(|e| ServerError::Protocol(e.to_string()))
}

fn dispatch_request(
    connection: &Connection,
    cfg: &ServerConfig,
    state: &mut State,
    req: &lsp_server::Request,
) -> Result<(), ServerError> {
    use lsp_types::{
        CodeActionRequest, CompletionRequest, DefinitionRequest, LspRequestMethod,
        ReferencesRequest, Request as _,
    };

    let method = LspRequestMethod::from(req.method.as_str());

    if method == DefinitionRequest::METHOD {
        return answer(
            connection,
            cfg,
            state,
            req,
            |analysis, params: lsp_types::DefinitionParams| {
                let at = params.text_document_position_params;
                analysis
                    .and_then(|a| definition::goto(a, &at.text_document.uri, at.position))
                    .map(|location| {
                        lsp_types::DefinitionResponse::Definition(lsp_types::Definition::Location(
                            location,
                        ))
                    })
            },
        );
    }

    if method == CompletionRequest::METHOD {
        return answer(
            connection,
            cfg,
            state,
            req,
            |analysis, params: lsp_types::CompletionParams| {
                let at = params.text_document_position_params;
                let items = analysis
                    .map(|a| completion::complete(a, &at.text_document.uri, at.position))
                    .unwrap_or_default();
                Some(lsp_types::CompletionResponse::CompletionItemList(items))
            },
        );
    }

    if method == ReferencesRequest::METHOD {
        return answer(
            connection,
            cfg,
            state,
            req,
            |analysis, params: lsp_types::ReferenceParams| {
                let at = params.text_document_position_params;
                Some(
                    analysis
                        .map(|a| references::find(a, &at.text_document.uri, at.position))
                        .unwrap_or_default(),
                )
            },
        );
    }

    if method == CodeActionRequest::METHOD {
        return answer(
            connection,
            cfg,
            state,
            req,
            |analysis, params: lsp_types::CodeActionParams| {
                Some(
                    analysis
                        .map(|a| code_action::actions(a, &params.text_document.uri, params.range))
                        .unwrap_or_default(),
                )
            },
        );
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
        fn discover_fragments(&self) -> Result<Vec<String>, ProviderError> {
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
