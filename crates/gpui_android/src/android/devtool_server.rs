//! Localhost HTTP server exposing the latest GPUI hitbox snapshot and
//! accepting synthetic taps. Bound to `127.0.0.1`; bridge from a developer
//! workstation via `adb reverse tcp:<port> tcp:<port>`.
//!
//! Endpoints:
//! - `GET  /ping`          → liveness
//! - `GET  /elements`      → current frame's interactive hitboxes
//! - `POST /tap`           → `{"element_id": "<path>"}` injects a tap at the
//!                           hitbox center. If multiple entries share the path,
//!                           the topmost (smallest bounds) wins.
//! - `POST /tap_xy`        → `{"x": f32, "y": f32}` raw fallback for untagged
//!                           regions.
//!
//! Bodies are flat JSON; parsed without serde to avoid pulling a dependency
//! into the renderer crate. All requests close after one round-trip.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Mutex;
use std::thread;

use gpui::devtool;

#[derive(Clone, Copy, Debug)]
pub(crate) struct PendingTap {
    pub x: f32,
    pub y: f32,
}

static TAP_QUEUE: Mutex<Vec<PendingTap>> = Mutex::new(Vec::new());

pub(crate) fn drain_pending_taps() -> Vec<PendingTap> {
    TAP_QUEUE
        .lock()
        .map(|mut q| std::mem::take(&mut *q))
        .unwrap_or_default()
}

fn enqueue_tap(x: f32, y: f32) {
    if let Ok(mut q) = TAP_QUEUE.lock() {
        q.push(PendingTap { x, y });
    }
}

pub fn start(port: u16) {
    let _ = thread::Builder::new()
        .name("devtool-http".into())
        .spawn(move || {
            let bind = format!("127.0.0.1:{port}");
            let listener = match TcpListener::bind(&bind) {
                Ok(l) => l,
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
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("");

    let mut content_length: usize = 0;
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
    }
    let mut body = vec![0u8; content_length];
    if content_length > 0 && reader.read_exact(&mut body).is_err() {
        return;
    }

    let (status, body_resp) = match (method, path) {
        ("GET", "/ping") => ("200 OK", String::from(r#"{"ok":true}"#)),
        ("GET", "/elements") => ("200 OK", elements_json()),
        ("POST", "/tap") => handle_tap(&body),
        ("POST", "/tap_xy") => handle_tap_xy(&body),
        _ => ("404 Not Found", String::from(r#"{"ok":false}"#)),
    };

    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n{body_resp}",
        len = body_resp.len()
    );
    let _ = stream.write_all(response.as_bytes());
}

fn elements_json() -> String {
    let snap = devtool::snapshot();
    let mut out = String::with_capacity(64 + snap.entries.len() * 96);
    out.push_str("{\"frame_id\":");
    out.push_str(&snap.frame_id.to_string());
    out.push_str(",\"entries\":[");
    for (i, e) in snap.entries.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&format!(
            "{{\"path\":\"{}\",\"instance\":{},\"x\":{:.2},\"y\":{:.2},\"w\":{:.2},\"h\":{:.2}}}",
            json_escape(&e.path),
            e.instance_id,
            e.x,
            e.y,
            e.width,
            e.height
        ));
    }
    out.push_str("]}");
    out
}

fn handle_tap(body: &[u8]) -> (&'static str, String) {
    let Ok(body_str) = std::str::from_utf8(body) else {
        return ("400 Bad Request", String::from(r#"{"ok":false}"#));
    };
    let path = match parse_string_field(body_str, "element_id") {
        Some(p) => p,
        None => {
            return (
                "400 Bad Request",
                String::from(r#"{"ok":false,"error":"element_id required"}"#),
            );
        }
    };
    let snap = devtool::snapshot();
    let exact = snap.entries.iter().filter(|e| e.path == path).count();
    let suffix = format!("/{path}");
    let topmost = snap
        .entries
        .iter()
        .filter(|e| e.path == path || (exact == 0 && (e.path.ends_with(&suffix) || e.path == path)))
        .min_by(|a, b| {
            (a.width * a.height)
                .partial_cmp(&(b.width * b.height))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    match topmost {
        Some(e) => {
            let cx = e.x + e.width / 2.0;
            let cy = e.y + e.height / 2.0;
            enqueue_tap(cx, cy);
            (
                "200 OK",
                format!(
                    r#"{{"ok":true,"x":{cx:.2},"y":{cy:.2},"frame_id":{}}}"#,
                    snap.frame_id
                ),
            )
        }
        None => (
            "404 Not Found",
            String::from(r#"{"ok":false,"error":"element not found"}"#),
        ),
    }
}

fn handle_tap_xy(body: &[u8]) -> (&'static str, String) {
    let Ok(body_str) = std::str::from_utf8(body) else {
        return ("400 Bad Request", String::from(r#"{"ok":false}"#));
    };
    let x = parse_number_field(body_str, "x");
    let y = parse_number_field(body_str, "y");
    match (x, y) {
        (Some(x), Some(y)) => {
            enqueue_tap(x, y);
            ("200 OK", format!(r#"{{"ok":true,"x":{x:.2},"y":{y:.2}}}"#))
        }
        _ => (
            "400 Bad Request",
            String::from(r#"{"ok":false,"error":"x and y required"}"#),
        ),
    }
}

fn parse_string_field(body: &str, field: &str) -> Option<String> {
    let key = format!("\"{field}\"");
    let idx = body.find(&key)?;
    let after = &body[idx + key.len()..];
    let colon = after.find(':')?;
    let after_colon = after[colon + 1..].trim_start();
    let after_quote = after_colon.strip_prefix('"')?;
    let end = after_quote.find('"')?;
    Some(after_quote[..end].to_string())
}

fn parse_number_field(body: &str, field: &str) -> Option<f32> {
    let key = format!("\"{field}\"");
    let idx = body.find(&key)?;
    let after = &body[idx + key.len()..];
    let colon = after.find(':')?;
    let after_colon = after[colon + 1..].trim_start();
    let end = after_colon
        .find(|c: char| c == ',' || c == '}' || c.is_whitespace())
        .unwrap_or(after_colon.len());
    after_colon[..end].trim().parse().ok()
}

fn json_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}
