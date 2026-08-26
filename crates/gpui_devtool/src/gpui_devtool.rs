//! Platform-neutral localhost HTTP server exposing the latest GPUI hitbox snapshot and
//! accepting synthetic gestures. Shared by `gpui_android`/`gpui_ios`; each platform crate owns
//! starting the server and dispatching drained gesture events into its own input plumbing.
//!
//! Bound to `127.0.0.1`; bridge via `adb forward` (Android) or `iproxy` (iOS).
//!
//! Every endpoint except `GET /ping` requires an `X-Devtool-Token` header (random, generated
//! per process start, written to `token_file_path(port)`) — any other local process (or a
//! webpage via DNS rebinding) could otherwise drive the app's real UI, or run whatever debug
//! function app code registered, via plain localhost access.
//!
//! Endpoints (see `docs/DEVTOOL.md` for full request/response shapes):
//! - `GET  /ping` — liveness + `pid`, since two builds (sim + device via iproxy) can both
//!   bind the same port and `/ping` would otherwise succeed either way silently.
//! - `GET  /elements` — current frame's interactive hitboxes (path + bounds).
//! - `POST /press`/`/long_press` — `{"element_id"}`, fires `on_press`/`on_long_press` on any
//!   `.id(...)`'d element with no declaration; topmost (smallest bounds) wins on a shared path.
//! - `POST /tap_xy` — `{"x","y"}` raw screen touch, no element resolution.
//! - `POST /call` — `{"name","params"}` runs a debug-only function app code registered via
//!   `App::register_devtool_action`, not tied to any UI element.
//! - `POST /sequence` — `{"steps":[...]}` runs press/long_press/tap_xy/call/wait_for_element
//!   steps in order, stopping at the first unresolvable one.
//!
//! All dispatching endpoints block until the gesture has actually played out (bounded
//! timeout) rather than returning the instant the request is queued — `"completed": false`
//! means unconfirmed, not "didn't happen."
//!
//! Bodies parse/build with real `serde_json::Value` (not typed structs — simple enough that
//! `.get(...)` is clearer than a derive) — handles escaped quotes/unicode/nesting correctly,
//! which a hand-rolled string parser (an earlier version of this file) didn't.

use std::collections::{HashSet, VecDeque};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, LazyLock, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use gpui::{GestureKind, devtool};
use rand::Rng;
use serde_json::{Value, json};

/// Shared secret checked on every request except `GET /ping` — bind-to-localhost alone isn't
/// enough since any local process could otherwise drive the app's real UI via simulated touches.
static TOKEN: OnceLock<String> = OnceLock::new();

fn generate_token() -> String {
    format!("{:032x}", rand::rng().random::<u128>())
}

/// Fixed `/tmp`, not `std::env::temp_dir()` (per-session `$TMPDIR` on macOS), so a plain shell
/// script can find it. A physical device's `/tmp` is sandboxed and not host-visible, so
/// `bridge-ios` also logs the token as a fallback.
fn token_file_path(port: u16) -> std::path::PathBuf {
    std::path::PathBuf::from(format!("/tmp/gpui-devtool-token-{port}"))
}

/// One step of a synthetic gesture, dispatched by the platform's per-frame
/// tick via its normal input pipeline (the same one real touches use).
#[derive(Clone, Copy, Debug)]
pub enum GestureEvent {
    Down(f32, f32),
    Move(f32, f32),
    Up(f32, f32),
}

struct ActiveGesture {
    steps: VecDeque<(GestureEvent, Duration)>,
    started_at: Instant,
    request_id: u64,
}

static ACTIVE_GESTURE: Mutex<Option<ActiveGesture>> = Mutex::new(None);
static GESTURE_QUEUE: Mutex<VecDeque<(u64, f32, f32, GestureKind)>> = Mutex::new(VecDeque::new());
static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);
static GESTURE_DONE: LazyLock<Mutex<HashSet<u64>>> = LazyLock::new(|| Mutex::new(HashSet::new()));
static GESTURE_CONDVAR: Condvar = Condvar::new();

/// Ceiling `wait_for_gesture` blocks for (longest legit gesture, long-press's 600ms, plus
/// slack). Gestures are strictly serialized, so timing out means stuck, not just slow.
const GESTURE_WAIT_TIMEOUT: Duration = Duration::from_millis(2000);

/// Enqueue a gesture at a point; plays out one at a time, queued behind any already in flight.
/// Returns a request id — pass to `wait_for_gesture` to block until it finishes.
///
/// Unlike `snapshot()`'s publish request (`has_pending_publish` in `gpui/src/devtool.rs`, OR'd
/// into the window's dirty-gate), this queue needs no equivalent wake-up: `drain_gesture_events`
/// runs in each platform's per-tick FFI trampoline *before* that dirty-gate is evaluated, and
/// both platforms currently drive it unconditionally while foregrounded, idle or not. If either
/// ever pauses that tick loop on idle, a queued gesture would have no fallback to force a
/// wake-up.
pub fn enqueue_gesture(x: f32, y: f32, kind: GestureKind) -> u64 {
    let id = NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed);
    if let Ok(mut queue) = GESTURE_QUEUE.lock() {
        queue.push_back((id, x, y, kind));
    }
    id
}

/// Block until gesture `id` fully plays out. `false` on timeout — caller's response should
/// reflect that rather than claim success on a gesture that may still be pending.
pub fn wait_for_gesture(id: u64) -> bool {
    let Ok(guard) = GESTURE_DONE.lock() else {
        return false;
    };
    let result =
        GESTURE_CONDVAR.wait_timeout_while(guard, GESTURE_WAIT_TIMEOUT, |done| !done.contains(&id));
    let Ok((mut guard, wait_result)) = result else {
        return false;
    };
    let completed = guard.remove(&id);
    completed && !wait_result.timed_out()
}

/// Drain events due to fire this tick. Call once per frame; a gesture spans many calls
/// (long-press, swipe), each returning whatever steps crossed their time offset.
pub fn drain_gesture_events() -> Vec<GestureEvent> {
    let mut fired = Vec::new();
    let now = Instant::now();
    let Ok(mut active) = ACTIVE_GESTURE.lock() else {
        return fired;
    };
    if active.is_none() {
        if let Ok(mut queue) = GESTURE_QUEUE.lock() {
            if let Some((request_id, x, y, kind)) = queue.pop_front() {
                *active = Some(ActiveGesture {
                    steps: plan_gesture(x, y, kind),
                    started_at: now,
                    request_id,
                });
            }
        }
    }
    if let Some(gesture) = active.as_mut() {
        while let Some((_, offset)) = gesture.steps.front() {
            if now.duration_since(gesture.started_at) < *offset {
                break;
            }
            let Some((event, _)) = gesture.steps.pop_front() else {
                break;
            };
            fired.push(event);
        }
        if gesture.steps.is_empty() {
            let request_id = gesture.request_id;
            *active = None;
            if let Ok(mut done) = GESTURE_DONE.lock() {
                done.insert(request_id);
            }
            GESTURE_CONDVAR.notify_all();
        }
    }
    fired
}

/// Precompute a gesture's event sequence as (event, time-since-start) pairs.
/// Swipe distance/duration are fixed defaults — realistic enough to trigger
/// swipe-driven UI without per-call tuning knobs.
fn plan_gesture(x: f32, y: f32, kind: GestureKind) -> VecDeque<(GestureEvent, Duration)> {
    const LONG_PRESS_HOLD_MS: u64 = 600;
    const PRESS_HOLD_MS: u64 = 30;

    let mut steps = VecDeque::new();
    match kind {
        GestureKind::Press => {
            steps.push_back((GestureEvent::Down(x, y), Duration::ZERO));
            steps.push_back((GestureEvent::Up(x, y), Duration::from_millis(PRESS_HOLD_MS)));
        }
        GestureKind::LongPress => {
            steps.push_back((GestureEvent::Down(x, y), Duration::ZERO));
            steps.push_back((
                GestureEvent::Up(x, y),
                Duration::from_millis(LONG_PRESS_HOLD_MS),
            ));
        }
    }
    steps
}

/// Start the devtool HTTP server on a background thread. Bind failure just logs (never panics).
///
/// A second call in the same process is a no-op: `TOKEN` is a `OnceLock`, so the running
/// server keeps checking the first-installed token regardless of what a later call generates —
/// writing a different token to the file/log on that call would desync the two.
pub fn start(port: u16) {
    let token = generate_token();
    if TOKEN.set(token.clone()).is_err() {
        log::warn!("devtool: start() called again in the same process; ignoring");
        return;
    }
    let token_path = token_file_path(port);
    if let Err(error) = std::fs::write(&token_path, &token) {
        log::error!("devtool: failed to write token file {token_path:?}: {error:?}");
    }
    // Also logged: `/tmp` isn't host-reachable from inside a physical iOS
    // device's sandbox, so a caller bridging to a device reads this back
    // from log capture instead of the file (see token_file_path docs).
    log::info!("devtool: token: {token}");

    let spawn_result = thread::Builder::new()
        .name("devtool-http".into())
        .spawn(move || {
            let bind = format!("127.0.0.1:{port}");
            let listener = match TcpListener::bind(&bind) {
                Ok(listener) => listener,
                Err(error) => {
                    log::error!("devtool: bind {bind} failed: {error:?}");
                    return;
                }
            };
            log::info!("devtool: listening on {bind}");
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                handle_request(&mut stream);
            }
        });
    if let Err(error) = spawn_result {
        log::error!("devtool: failed to spawn HTTP thread: {error:?}");
    }
}

fn handle_request(stream: &mut TcpStream) {
    let Ok(clone) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(clone);

    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        return;
    }
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("").to_string();

    let mut content_length: usize = 0;
    let mut token_header: Option<String> = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() {
            break;
        }
        if line == "\r\n" || line.is_empty() {
            break;
        }
        let lower = line.to_ascii_lowercase();
        if let Some(rest) = lower.strip_prefix("content-length:") {
            content_length = rest.trim().parse().unwrap_or(0);
        }
        if lower.starts_with("x-devtool-token:") {
            if let Some(colon) = line.find(':') {
                token_header = Some(line[colon + 1..].trim().to_string());
            }
        }
    }
    let mut body = vec![0u8; content_length];
    if content_length > 0 && reader.read_exact(&mut body).is_err() {
        return;
    }

    if path != "/ping" {
        let authorized = TOKEN
            .get()
            .is_none_or(|expected| token_header.as_deref().is_some_and(|got| got == expected));
        if !authorized {
            let body_resp =
                json!({"ok": false, "error": "missing or invalid X-Devtool-Token header"})
                    .to_string();
            let response = format!(
                "HTTP/1.1 401 Unauthorized\r\nContent-Type: application/json\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n{body_resp}",
                len = body_resp.len()
            );
            let _ = stream.write_all(response.as_bytes());
            return;
        }
    }
    let method = method.as_str();
    let path = path.as_str();

    let (status, body_resp) = match (method, path) {
        ("GET", "/ping") => (
            "200 OK",
            json!({"ok": true, "pid": std::process::id()}).to_string(),
        ),
        ("GET", "/elements") => ("200 OK", elements_json()),
        ("POST", "/press") => handle_gesture(&body, GestureKind::Press),
        ("POST", "/long_press") => handle_gesture(&body, GestureKind::LongPress),
        ("POST", "/tap_xy") => handle_tap_xy(&body),
        ("POST", "/call") => handle_call(&body),
        ("POST", "/sequence") => handle_sequence(&body),
        _ => ("404 Not Found", json!({"ok": false}).to_string()),
    };

    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n{body_resp}",
        len = body_resp.len()
    );
    let _ = stream.write_all(response.as_bytes());
}

fn entry_json(entry: &devtool::Entry) -> Value {
    json!({
        "path": entry.path,
        "instance": entry.instance_id,
        "x": entry.x,
        "y": entry.y,
        "w": entry.width,
        "h": entry.height,
    })
}

fn elements_json() -> String {
    let snap = devtool::snapshot();
    json!({
        "frame_id": snap.frame_id,
        "pid": std::process::id(),
        "entries": snap.entries.iter().map(entry_json).collect::<Vec<_>>(),
    })
    .to_string()
}

/// Resolve an element_id (bare leaf or full slash path) to its topmost
/// (smallest-area) matching entry in the latest snapshot. Shared by every endpoint that
/// addresses an element by path (`/press`, `/long_press`, `wait_for_element`).
fn resolve_element(snap: &devtool::Snapshot, path: &str) -> Option<devtool::Entry> {
    let exact = snap.entries.iter().filter(|e| e.path == path).count();
    let suffix = format!("/{path}");
    snap.entries
        .iter()
        .filter(|e| e.path == path || (exact == 0 && (e.path.ends_with(&suffix) || e.path == path)))
        .min_by(|a, b| {
            (a.width * a.height)
                .partial_cmp(&(b.width * b.height))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .cloned()
}

/// Parse a request body as JSON. `None` on malformed/empty bodies — callers turn that into a 400.
fn parse_body(body: &[u8]) -> Option<Value> {
    serde_json::from_slice(body).ok()
}

/// Core of `/press`/`/long_press`. `ok` is `false` only when the element itself couldn't be
/// resolved (`/sequence` uses this to decide whether to stop); a timed-out gesture is still
/// `ok: true` here (see `"completed"` in the body).
fn do_gesture_by_id(element_id: &str, kind: GestureKind) -> (bool, Value) {
    let snap = devtool::snapshot();
    match resolve_element(&snap, element_id) {
        Some(e) => {
            let cx = e.x + e.width / 2.0;
            let cy = e.y + e.height / 2.0;
            let id = enqueue_gesture(cx, cy, kind);
            let completed = wait_for_gesture(id);
            (
                true,
                json!({"ok": true, "completed": completed, "x": cx, "y": cy, "frame_id": snap.frame_id}),
            )
        }
        None => (false, json!({"ok": false, "error": "element not found"})),
    }
}

/// Core of `/tap_xy`.
fn do_tap_xy(x: f32, y: f32) -> (bool, Value) {
    let id = enqueue_gesture(x, y, GestureKind::Press);
    let completed = wait_for_gesture(id);
    (
        true,
        json!({"ok": true, "completed": completed, "x": x, "y": y}),
    )
}

/// Cap on `wait_for_element`'s block, even if asked for more — the server is single-threaded,
/// so an unbounded wait would make it unresponsive to every other caller.
const MAX_WAIT_FOR_ELEMENT_MS: u64 = 10_000;

/// Core of `/sequence`'s `wait_for_element` step. Polls the snapshot (cheap in-memory clone)
/// instead of requiring the caller to sleep-then-repoll. Checks existence only — no access to
/// the external log capture file, so log-pattern waiting stays client-side (`ios-log.sh wait`).
fn do_wait_for_element(element_id: &str, timeout_ms: u64) -> (bool, Value) {
    let timeout_ms = timeout_ms.min(MAX_WAIT_FOR_ELEMENT_MS);
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        let snap = devtool::snapshot();
        if resolve_element(&snap, element_id).is_some() {
            return (true, json!({"ok": true, "found": true}));
        }
        if Instant::now() >= deadline {
            return (
                false,
                json!({"ok": false, "found": false, "error": "timeout waiting for element"}),
            );
        }
        thread::sleep(Duration::from_millis(50));
    }
}

/// Shared by `/press` and `/long_press`, which differ only in `GestureKind`.
fn handle_gesture(body: &[u8], kind: GestureKind) -> (&'static str, String) {
    let Some(request) = parse_body(body) else {
        return (
            "400 Bad Request",
            json!({"ok": false, "error": "invalid JSON body"}).to_string(),
        );
    };
    let Some(path) = request.get("element_id").and_then(Value::as_str) else {
        return (
            "400 Bad Request",
            json!({"ok": false, "error": "element_id required"}).to_string(),
        );
    };
    let (ok, result) = do_gesture_by_id(path, kind);
    (
        if ok { "200 OK" } else { "404 Not Found" },
        result.to_string(),
    )
}

fn handle_tap_xy(body: &[u8]) -> (&'static str, String) {
    let Some(request) = parse_body(body) else {
        return (
            "400 Bad Request",
            json!({"ok": false, "error": "invalid JSON body"}).to_string(),
        );
    };
    let x = request.get("x").and_then(Value::as_f64).map(|v| v as f32);
    let y = request.get("y").and_then(Value::as_f64).map(|v| v as f32);
    match (x, y) {
        (Some(x), Some(y)) => {
            let (_, result) = do_tap_xy(x, y);
            ("200 OK", result.to_string())
        }
        _ => (
            "400 Bad Request",
            json!({"ok": false, "error": "x and y required"}).to_string(),
        ),
    }
}

/// Core of `/call` — queues `name`/`params` for `Window::draw` to look up in the app's
/// `ActionRegistry` global and run, then blocks until it has actually run.
fn do_call(name: &str, params: Value) -> (bool, Value) {
    let id = devtool::enqueue_call(name.to_string(), params);
    let outcome = devtool::wait_for_call(id);
    (
        outcome.invoked,
        json!({
            "ok": true,
            "completed": outcome.completed,
            "invoked": outcome.invoked,
            "result": outcome.result,
        }),
    )
}

fn handle_call(body: &[u8]) -> (&'static str, String) {
    let Some(request) = parse_body(body) else {
        return (
            "400 Bad Request",
            json!({"ok": false, "error": "invalid JSON body"}).to_string(),
        );
    };
    let Some(name) = request.get("name").and_then(Value::as_str) else {
        return (
            "400 Bad Request",
            json!({"ok": false, "error": "name required"}).to_string(),
        );
    };
    let params = request.get("params").cloned().unwrap_or(Value::Null);
    let (ok, result) = do_call(name, params);
    (
        if ok { "200 OK" } else { "404 Not Found" },
        result.to_string(),
    )
}

/// `POST /sequence` steps, executed in order. Stops at the first step whose element itself
/// couldn't be resolved (a gesture timeout still counts as progress).
fn handle_sequence(body: &[u8]) -> (&'static str, String) {
    let Some(request) = parse_body(body) else {
        return (
            "400 Bad Request",
            json!({"ok": false, "error": "invalid JSON body"}).to_string(),
        );
    };
    let Some(steps) = request.get("steps").and_then(Value::as_array) else {
        return (
            "400 Bad Request",
            json!({"ok": false, "error": "steps (array) required"}).to_string(),
        );
    };
    if steps.is_empty() {
        return (
            "400 Bad Request",
            json!({"ok": false, "error": "steps must not be empty"}).to_string(),
        );
    }

    let mut results: Vec<Value> = Vec::with_capacity(steps.len());
    let mut all_ok = true;
    for step in steps {
        let step_type = step.get("type").and_then(Value::as_str).unwrap_or("");
        let (ok, mut result) = match step_type {
            "press" => match step.get("element_id").and_then(Value::as_str) {
                Some(id) => do_gesture_by_id(id, GestureKind::Press),
                None => (false, json!({"ok": false, "error": "element_id required"})),
            },
            "long_press" => match step.get("element_id").and_then(Value::as_str) {
                Some(id) => do_gesture_by_id(id, GestureKind::LongPress),
                None => (false, json!({"ok": false, "error": "element_id required"})),
            },
            "tap_xy" => {
                let x = step.get("x").and_then(Value::as_f64).map(|v| v as f32);
                let y = step.get("y").and_then(Value::as_f64).map(|v| v as f32);
                match (x, y) {
                    (Some(x), Some(y)) => do_tap_xy(x, y),
                    _ => (false, json!({"ok": false, "error": "x and y required"})),
                }
            }
            "wait_for_element" => {
                let element_id = step.get("element_id").and_then(Value::as_str);
                let timeout_ms = step
                    .get("timeout_ms")
                    .and_then(Value::as_u64)
                    .unwrap_or(2000);
                match element_id {
                    Some(id) => do_wait_for_element(id, timeout_ms),
                    None => (false, json!({"ok": false, "error": "element_id required"})),
                }
            }
            "call" => match step.get("name").and_then(Value::as_str) {
                Some(name) => do_call(name, step.get("params").cloned().unwrap_or(Value::Null)),
                None => (false, json!({"ok": false, "error": "name required"})),
            },
            other => (
                false,
                json!({"ok": false, "error": format!("unknown step type: {other:?}")}),
            ),
        };
        result["type"] = json!(step_type);
        results.push(result);
        if !ok {
            all_ok = false;
            break;
        }
    }

    (
        "200 OK",
        json!({
            "ok": all_ok,
            "steps_completed": results.len(),
            "steps_total": steps.len(),
            "results": results,
        })
        .to_string(),
    )
}
