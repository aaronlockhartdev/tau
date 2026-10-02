use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::scenario::{ScenarioSet, WireRequest, render_frames};

/// Frame pacing: the GUI coalesces stream deltas at 25 ms (spec §8), so
/// the mock streams at the same cadence — the E2E measures incremental
/// rendering, not one-shot bodies.
const FRAME_GAP: Duration = Duration::from_millis(25);

/// The model id the mock advertises; the suites configure their provider
/// entries with it.
pub const MODEL_ID: &str = "mock-model";

pub async fn serve(listener: TcpListener, set: Arc<ScenarioSet>) {
    loop {
        let Ok((sock, _)) = listener.accept().await else {
            return;
        };
        tokio::spawn(handle(sock, set.clone()));
    }
}

async fn handle(mut sock: TcpStream, set: Arc<ScenarioSet>) {
    let Some((method, path, body)) = read_request(&mut sock).await else {
        return;
    };
    if method == "GET" && path.ends_with("/models") {
        let body = json!({ "data": [{ "id": MODEL_ID }] }).to_string();
        let _ = reply(&mut sock, 200, "application/json", &body).await;
        return;
    }
    if method == "POST" && path.ends_with("/responses") {
        stream_response(&mut sock, &set, &body).await;
        return;
    }
    let _ = reply(&mut sock, 404, "text/plain", "not found").await;
}

/// Head up to the blank line, then the body up to `content-length`.
/// `None` on a closed/oversized connection.
async fn read_request(sock: &mut TcpStream) -> Option<(String, String, Vec<u8>)> {
    // Read until the blank line — but a single TCP segment usually carries
    // head *and* body, so keep the excess bytes instead of dropping them.
    let mut buf = Vec::new();
    let mut tmp = [0u8; 8192];
    let sep = loop {
        let n = sock.read(&mut tmp).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&tmp[..n]);
        if buf.len() > 10 * 1024 * 1024 {
            return None;
        }
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
    };
    let head = String::from_utf8_lossy(&buf[..sep]).into_owned();
    let mut body = buf[sep + 4..].to_vec();
    let mut lines = head.lines();
    let request_line = lines.next().unwrap_or_default().to_owned();
    let mut content_length = 0usize;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.trim().eq_ignore_ascii_case("content-length") {
            content_length = value.trim().parse().unwrap_or(0);
        }
    }
    let mut parts = request_line.split_whitespace();
    let (method, path) = match (parts.next(), parts.next()) {
        (Some(m), Some(p)) => (m.to_owned(), p.to_owned()),
        _ => return None,
    };
    if content_length > 10 * 1024 * 1024 {
        return None;
    }
    while body.len() < content_length {
        let n = sock.read(&mut tmp).await.ok()?;
        if n == 0 {
            return None;
        }
        body.extend_from_slice(&tmp[..n]);
    }
    body.truncate(content_length);
    Some((method, path, body))
}

/// Select the scenario, advance its turn, and stream the frames.
async fn stream_response(sock: &mut TcpStream, set: &ScenarioSet, body: &[u8]) {
    let req: WireRequest = serde_json::from_slice(body).unwrap_or_default();
    let scenario = set.select(&req);
    let (seq, turn) = scenario.next_turn();
    // One line per request: the e2e log carries the routing, so a spec
    // failure shows which scenario answered and with which turn.
    eprintln!("[route] {} turn {}", scenario.pattern, seq);
    let frames = render_frames(turn, &scenario.usage, seq);
    let _ = reply_headers(sock).await;
    for frame in frames {
        let _ = sock
            .write_all(format!("data: {frame}\n\n").as_bytes())
            .await;
        let _ = sock.flush().await;
        tokio::time::sleep(FRAME_GAP).await;
    }
}

async fn reply_headers(sock: &mut TcpStream) -> std::io::Result<()> {
    sock.write_all(
        b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n",
    )
    .await
}

async fn reply(sock: &mut TcpStream, status: u16, ctype: &str, body: &str) -> std::io::Result<()> {
    let head = format!(
        "HTTP/1.1 {status} X\r\ncontent-type: {ctype}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        body.len()
    );
    sock.write_all(head.as_bytes()).await?;
    sock.write_all(body.as_bytes()).await
}
