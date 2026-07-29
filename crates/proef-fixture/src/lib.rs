//! The proef fixture API server (TESTING-STRATEGY): a synchronous in-process
//! backend the integration suite runs the ported 50x corpus against.
//!
//! Determinism rule: delayed visibility is **token-driven** (poll counters),
//! never sleep-raced — a resource becomes visible on the Nth poll, so retry
//! tests assert attempt counts, not wall time. The one deliberate exception is
//! `/slow`, which sleeps to exercise budgets/watchdog.
//!
//! Auth: `Authorization: Bearer fixture-token` on `/api/v1/*` admin routes;
//! per-feed read routes are unauthenticated (the sync-consumer side).

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
    push_polls: u32,
    messages: Vec<String>,
    events: Vec<String>,
    photos: Vec<String>,
    call: Option<(String, &'static str)>,
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

    // Per-feed read/interaction routes are unauthenticated (sync-consumer
    // side); everything else under /api/v1/ needs the bearer.
    let feed_read_side = path.contains("/feed/") && !path.ends_with("/activate");
    let needs_auth = path.starts_with("/api/v1/") && !feed_read_side;
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
        ("POST", "/api/v1/feed/activate") => {
            st.ready_polls = 0;
            respond_json(request, 200, r#"{"feedId":"feed-1"}"#);
        }
        ("GET", "/api/v1/feed/feed-1/state") => {
            st.ready_polls += 1;
            let status = if st.ready_polls >= VISIBLE_AFTER {
                "ready"
            } else {
                "provisioning"
            };
            respond_json(request, 200, &format!(r#"{{"status":"{status}"}}"#));
        }
        ("GET", "/api/v1/admin/search/clients") => {
            respond_json(request, 200, r#"[{"id":"c-1","name":"Bakker"}]"#);
        }
        ("GET", "/api/v1/admin/search/users") => {
            respond_json(request, 200, r#"[{"id":"u-1","name":"de Vries"}]"#);
        }
        ("GET", "/api/v1/admin/search/missing") => {
            respond_json(request, 404, r#"{"error":"unknown index"}"#);
        }
        ("GET", "/api/v1/clients/c-1") => respond_json(request, 200, r#"{"id":"c-1"}"#),
        ("POST", "/api/v1/clients/c-1/messages") => {
            st.messages.push("m-1".to_owned());
            st.push_polls = 0;
            respond_json(request, 201, r#"{"id":"m-1"}"#);
        }
        ("GET", "/api/v1/push/queue") => {
            st.push_polls += 1;
            let items = if st.push_polls >= VISIBLE_AFTER {
                r#"[{"kind":"push"}]"#
            } else {
                "[]"
            };
            respond_json(request, 200, &format!(r#"{{"items":{items}}}"#));
        }
        ("GET", "/api/v1/feed/feed-1/messages") => {
            let ids: Vec<String> = st
                .messages
                .iter()
                .map(|m| format!(r#"{{"id":"{m}"}}"#))
                .collect();
            respond_json(
                request,
                200,
                &format!(r#"{{"messages":[{}]}}"#, ids.join(",")),
            );
        }
        ("POST", "/api/v1/clients/c-1/agenda/events") => {
            st.events.push("e-1".to_owned());
            st.push_polls = 0;
            respond_json(request, 201, r#"{"id":"e-1"}"#);
        }
        ("GET", "/api/v1/feed/feed-1/calendar") => {
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
        ("POST", "/api/v1/clients/c-1/photos") => {
            st.photos.push("p-1".to_owned());
            st.push_polls = 0;
            respond_json(request, 201, r#"{"id":"p-1"}"#);
        }
        ("GET", "/api/v1/feed/feed-1/photos") => {
            let ids: Vec<String> = st
                .photos
                .iter()
                .map(|p| format!(r#"{{"id":"{p}"}}"#))
                .collect();
            respond_json(
                request,
                200,
                &format!(r#"{{"photos":[{}]}}"#, ids.join(",")),
            );
        }
        ("POST", "/api/v1/clients/c-1/calls") => {
            st.call = Some(("call-1".to_owned(), "incoming"));
            respond_json(request, 201, r#"{"id":"call-1"}"#);
        }
        ("GET", "/api/v1/feed/feed-1/call") => {
            let (id, call_state) = st
                .call
                .as_ref()
                .map_or(("", "idle"), |(id, s)| (id.as_str(), *s));
            respond_json(
                request,
                200,
                &format!(r#"{{"state":"{call_state}","callId":"{id}"}}"#),
            );
        }
        ("POST", "/api/v1/feed/feed-1/call/call-1/deny") => {
            if let Some(call) = st.call.as_mut() {
                call.1 = "idle";
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
        ("GET", "/slow") => {
            // The deliberate sleep: exercises budgets + watchdog (ADR-0007).
            std::thread::sleep(Duration::from_secs(5));
            respond_json(request, 200, r#"{"finally":true}"#);
        }
        ("POST", "/upload") => respond_json(request, 201, r#"{"id":"ph-1"}"#),
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
