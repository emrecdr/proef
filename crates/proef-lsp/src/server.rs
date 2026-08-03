//! The stdio LSP event loop: handshake, dispatch, and the (later) debounced
//! recompute driver. Single-threaded — the analysis is milliseconds, so v1
//! needs no worker pool.

use lsp_server::{Connection, IoThreads, Message};
use lsp_types::ServerCapabilities;

use crate::documents::Documents;

/// How the server talks to its client.
pub enum Transport {
    /// Real stdio (production).
    Stdio,
    /// An in-process connection (tests).
    InMemory(Connection),
}

/// Startup configuration for [`run`].
pub struct ServerConfig {
    /// How the server talks to its client.
    pub transport: Transport,
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
/// no capability flag; definition/completion/references are filled in as their
/// tasks land. Text sync is FULL (we recompute wholesale anyway).
fn capabilities() -> ServerCapabilities {
    use lsp_types::{TextDocumentSyncCapability, TextDocumentSyncKind};
    ServerCapabilities {
        text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
        ..Default::default()
    }
}

/// Runs the LSP server to completion: blocks on the `initialize` handshake,
/// dispatches messages until `shutdown`/`exit`, then joins the transport.
pub fn run(cfg: ServerConfig) -> Result<(), ServerError> {
    let (connection, io_threads): (Connection, Option<IoThreads>) = match cfg.transport {
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
    // Blocks until the client's `initialize`; returns its params (unused in v1
    // beyond the handshake — the workspace root is discovered from open docs).
    let _init_params = connection
        .initialize(caps)
        .map_err(|e| ServerError::Protocol(e.to_string()))?;

    let mut docs = Documents::default();
    main_loop(&connection, &mut docs)?;

    if let Some(threads) = io_threads {
        threads.join().map_err(ServerError::Io)?;
    }
    Ok(())
}

fn main_loop(connection: &Connection, _docs: &mut Documents) -> Result<(), ServerError> {
    for msg in &connection.receiver {
        match msg {
            Message::Request(req) => {
                if connection
                    .handle_shutdown(&req)
                    .map_err(|e| ServerError::Protocol(e.to_string()))?
                {
                    return Ok(());
                }
                // Feature requests (definition/completion/references) are wired
                // in their own tasks; unknown methods get a method-not-found
                // response so the client never hangs.
                let resp = lsp_server::Response::new_err(
                    req.id.clone(),
                    lsp_server::ErrorCode::MethodNotFound as i32,
                    format!("unhandled request: {}", req.method),
                );
                connection
                    .sender
                    .send(Message::Response(resp))
                    .map_err(|e| ServerError::Protocol(e.to_string()))?;
            }
            Message::Notification(_note) => {
                // didOpen/didChange/didClose handling lands with diagnostics.
            }
            Message::Response(_) => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use lsp_server::{Connection, Message, Notification, Request, RequestId};
    use lsp_types::{InitializeParams, InitializedParams};

    #[test]
    fn initialize_then_shutdown_completes_cleanly() {
        // `Connection::memory()` gives us both ends in-process; we drive the
        // client end, the server logic runs on the other.
        let (server_conn, client) = Connection::memory();

        let server = std::thread::spawn(move || {
            run(ServerConfig {
                transport: Transport::InMemory(server_conn),
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
