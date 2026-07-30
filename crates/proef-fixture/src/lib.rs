//! The proef fixture API server (TESTING-STRATEGY): a synchronous in-process
//! backend the integration suite runs the reference corpus against.
//!
//! The domain is a deliberately generic **workspace / activity board**: a
//! member posts items (a note, a scheduled event, an attachment, a live
//! session) to a record, and they surface on the record's synced activity
//! channel. It exercises proef's mechanics (create + poll-until-visible,
//! search, multipart, action + cancel), nothing product-specific.
//!
//! Determinism rule: delayed visibility is **token-driven** (poll counters),
//! never sleep-raced — a resource becomes visible on the Nth poll, so retry
//! tests assert attempt counts, not wall time. The one deliberate exception is
//! `/slow`, which sleeps to exercise budgets/watchdog.
//!
//! Auth: `Authorization: Bearer fixture-token` on `/api/v1/*` admin routes;
//! per-channel read routes are unauthenticated (the sync-consumer side).
//!
//! **Single-threaded by design**: one accept loop, one request at a time —
//! request handling needs no internal synchronization discipline beyond the
//! `Envs` mutex, and suite wall-time is bounded by the poll-token rule above.
//! `/slow` is served before the state lock so the deliberate sleep can never
//! wedge other routes if worker threads are ever added.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The bearer token the fixture accepts (tests export
/// `PROEF_SECRET_APITOKEN=fixture-token`).
pub const API_TOKEN: &str = "fixture-token";

/// How many polls until a delayed resource becomes visible.
const VISIBLE_AFTER: u32 = 2;

/// Per-environment state (parallel scenarios are isolated by the
/// `X-Proef-Env` header their packs send after provisioning).
#[derive(Default)]
struct Envs {
    counter: u32,
    envs: HashMap<String, State>,
}

#[derive(Default)]
struct State {
    ready_polls: u32,
    delivery_polls: u32,
    notes: Vec<String>,
    events: Vec<String>,
    attachments: Vec<String>,
    session: Option<(String, &'static str)>,
}

/// A running fixture server (drops = shutdown + join).
pub struct Fixture {
    /// Base URL (`http://127.0.0.1:<port>`).
    pub base_url: String,
    shutdown: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Fixture {
    /// Start on an ephemeral port.
    pub fn start() -> Result<Self, String> {
        let server = tiny_http::Server::http("127.0.0.1:0")
            .map_err(|err| format!("cannot start fixture: {err}"))?;
        // `to_ip()` instead of matching: the `Unix` variant only exists on
        // unix targets, so a match arm would not compile on Windows.
        let port = server
            .server_addr()
            .to_ip()
            .ok_or_else(|| "fixture bound to a non-IP address".to_owned())?
            .port();
        let shutdown = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&shutdown);
        let handle = std::thread::spawn(move || {
            let state = Mutex::new(Envs::default());
            while !flag.load(Ordering::SeqCst) {
                match server.recv_timeout(Duration::from_millis(50)) {
                    Ok(Some(request)) => handle_request(request, &state),
                    Ok(None) => {}
                    Err(_) => break,
                }
            }
        });
        Ok(Self {
            base_url: format!("http://127.0.0.1:{port}"),
            shutdown,
            handle: Some(handle),
        })
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

// One flat route table — splitting it would obscure the API surface.
#[allow(clippy::too_many_lines)]
fn handle_request(mut request: tiny_http::Request, state: &Mutex<Envs>) {
    let mut body = String::new();
    let _ = std::io::Read::read_to_string(request.as_reader(), &mut body);
    let method = request.method().as_str().to_owned();
    let url = request.url().to_owned();
    let (path, _query) = url.split_once('?').unwrap_or((url.as_str(), ""));

    let authed = request.headers().iter().any(|h| {
        h.field
            .as_str()
            .as_str()
            .eq_ignore_ascii_case("authorization")
            && h.value.as_str() == format!("Bearer {API_TOKEN}")
    });
    let has_cookie = request.headers().iter().any(|h| {
        h.field.as_str().as_str().eq_ignore_ascii_case("cookie")
            && h.value.as_str().contains("proefsession=abc123")
    });

    // Per-channel read/interaction routes are unauthenticated (sync-consumer
    // side); everything else under /api/v1/ needs the bearer.
    let channel_read_side = path.contains("/channel/") && !path.ends_with("/activate");
    let needs_auth = path.starts_with("/api/v1/") && !channel_read_side;
    if needs_auth && !authed {
        respond_json(request, 401, r#"{"error":"unauthorized"}"#);
        return;
    }

    let env_key = request
        .headers()
        .iter()
        .find(|h| {
            h.field
                .as_str()
                .as_str()
                .eq_ignore_ascii_case("x-proef-env")
        })
        .map_or_else(|| "default".to_owned(), |h| h.value.as_str().to_owned());

    if method == "GET" && path == "/slow" {
        // The deliberate sleep: exercises budgets + watchdog (ADR-0007).
        // Handled *before* the state lock — sleeping inside the guard would
        // wedge every route the moment this server grows worker threads.
        std::thread::sleep(Duration::from_secs(5));
        respond_json(request, 200, r#"{"finally":true}"#);
        return;
    }
    let Ok(mut envs) = state.lock() else {
        respond_json(request, 500, r#"{"error":"lock"}"#);
        return;
    };
    if method == "POST" && path == "/api/v1/env/provision" {
        envs.counter += 1;
        let id = format!("env-{}", envs.counter);
        envs.envs.insert(id.clone(), State::default());
        drop(envs);
        respond_json(request, 201, &format!(r#"{{"id":"{id}"}}"#));
        return;
    }
    let st = envs.envs.entry(env_key).or_default();

    match (method.as_str(), path) {
        ("GET", "/health") | ("POST", "/form") => {
            respond_json(request, 200, r#"{"status":"ok"}"#);
        }
        ("POST", "/api/v1/channel/activate") => {
            st.ready_polls = 0;
            respond_json(request, 200, r#"{"channelId":"ch-1"}"#);
        }
        ("GET", "/api/v1/channel/ch-1/state") => {
            st.ready_polls += 1;
            let status = if st.ready_polls >= VISIBLE_AFTER {
                "ready"
            } else {
                "provisioning"
            };
            respond_json(request, 200, &format!(r#"{{"status":"{status}"}}"#));
        }
        ("GET", "/api/v1/admin/search/records") => {
            respond_json(request, 200, r#"[{"id":"r-1","name":"Acme"}]"#);
        }
        ("GET", "/api/v1/admin/search/people") => {
            respond_json(request, 200, r#"[{"id":"pe-1","name":"Jordan Lee"}]"#);
        }
        ("GET", "/api/v1/admin/search/missing") => {
            respond_json(request, 404, r#"{"error":"unknown index"}"#);
        }
        ("GET", "/api/v1/records/r-1") => respond_json(request, 200, r#"{"id":"r-1"}"#),
        ("POST", "/api/v1/records/r-1/notes") => {
            st.notes.push("n-1".to_owned());
            st.delivery_polls = 0;
            respond_json(request, 201, r#"{"id":"n-1"}"#);
        }
        ("GET", "/api/v1/deliveries/queue") => {
            st.delivery_polls += 1;
            let items = if st.delivery_polls >= VISIBLE_AFTER {
                r#"[{"kind":"delivery"}]"#
            } else {
                "[]"
            };
            respond_json(request, 200, &format!(r#"{{"items":{items}}}"#));
        }
        ("GET", "/api/v1/channel/ch-1/notes") => {
            let ids: Vec<String> = st
                .notes
                .iter()
                .map(|n| format!(r#"{{"id":"{n}"}}"#))
                .collect();
            respond_json(request, 200, &format!(r#"{{"notes":[{}]}}"#, ids.join(",")));
        }
        ("POST", "/api/v1/records/r-1/events") => {
            st.events.push("e-1".to_owned());
            st.delivery_polls = 0;
            respond_json(request, 201, r#"{"id":"e-1"}"#);
        }
        ("GET", "/api/v1/channel/ch-1/events") => {
            let ids: Vec<String> = st
                .events
                .iter()
                .map(|e| format!(r#"{{"id":"{e}"}}"#))
                .collect();
            respond_json(
                request,
                200,
                &format!(r#"{{"events":[{}]}}"#, ids.join(",")),
            );
        }
        ("POST", "/api/v1/records/r-1/attachments") => {
            st.attachments.push("a-1".to_owned());
            st.delivery_polls = 0;
            respond_json(request, 201, r#"{"id":"a-1"}"#);
        }
        ("GET", "/api/v1/channel/ch-1/attachments") => {
            let ids: Vec<String> = st
                .attachments
                .iter()
                .map(|a| format!(r#"{{"id":"{a}"}}"#))
                .collect();
            respond_json(
                request,
                200,
                &format!(r#"{{"attachments":[{}]}}"#, ids.join(",")),
            );
        }
        ("POST", "/api/v1/records/r-1/sessions") => {
            st.session = Some(("s-1".to_owned(), "pending"));
            respond_json(request, 201, r#"{"id":"s-1"}"#);
        }
        ("GET", "/api/v1/channel/ch-1/session") => {
            let (id, session_state) = st
                .session
                .as_ref()
                .map_or(("", "idle"), |(id, s)| (id.as_str(), *s));
            respond_json(
                request,
                200,
                &format!(r#"{{"state":"{session_state}","sessionId":"{id}"}}"#),
            );
        }
        ("POST", "/api/v1/channel/ch-1/session/s-1/cancel") => {
            if let Some(session) = st.session.as_mut() {
                session.1 = "idle";
            }
            respond_json(request, 200, r#"{"ok":true}"#);
        }
        ("GET", "/cookie/set") => {
            let response = json_response(200, r#"{"cookie":"set"}"#).with_header(
                "Set-Cookie: proefsession=abc123; Path=/"
                    .parse::<tiny_http::Header>()
                    .unwrap_or_else(|()| unreachable!("static header")),
            );
            let _ = request.respond(response);
        }
        ("GET", "/cookie/check") => {
            if has_cookie {
                respond_json(request, 200, r#"{"cookie":"present"}"#);
            } else {
                respond_json(request, 403, r#"{"error":"no session cookie"}"#);
            }
        }
        ("POST", "/upload") => respond_json(request, 201, r#"{"id":"up-1"}"#),
        ("GET", "/malformed") => respond_json(request, 200, r#"{"broken": "#),
        _ => respond_json(request, 404, r#"{"error":"no such route"}"#),
    }
}

fn json_response(status: u16, body: &str) -> tiny_http::Response<std::io::Cursor<Vec<u8>>> {
    tiny_http::Response::from_string(body)
        .with_status_code(status)
        .with_header(
            "Content-Type: application/json"
                .parse::<tiny_http::Header>()
                .unwrap_or_else(|()| unreachable!("static header")),
        )
}

fn respond_json(request: tiny_http::Request, status: u16, body: &str) {
    let _ = request.respond(json_response(status, body));
}
