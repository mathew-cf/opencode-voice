//! SSE event stream client for OpenCode's /event endpoint.

use anyhow::Result;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use reqwest::Client;
use std::time::Duration;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;

use crate::approval::types::{PermissionRequest, QuestionRequest};

/// Events received from the OpenCode SSE stream.
#[derive(Debug, Clone)]
pub enum SseEvent {
    PermissionAsked(PermissionRequest),
    PermissionReplied {
        session_id: String,
        request_id: String,
        reply: String,
    },
    QuestionAsked(QuestionRequest),
    QuestionReplied {
        session_id: String,
        request_id: String,
        answers: Vec<Vec<String>>,
    },
    QuestionRejected {
        session_id: String,
        request_id: String,
    },
    /// Session status changed (busy/idle/retry).
    ///
    /// When the session becomes busy it means the AI has resumed work — any
    /// pending permissions or questions must have been answered (possibly by
    /// the user interacting directly with the OpenCode TUI).
    SessionStatus {
        session_id: String,
        /// `true` when the session is actively processing ("busy"),
        /// `false` when idle.
        busy: bool,
    },
    /// Session was updated (renamed, etc.).
    SessionUpdated { session_id: String },
    /// A new session was created.
    SessionCreated { session_id: String },
    /// A session was deleted.
    SessionDeleted { session_id: String },
    Connected,
    Disconnected(Option<String>),
}

/// SSE stream client with automatic reconnection.
pub struct OpenCodeEvents {
    base_url: String,
    password: Option<String>,
    sender: tokio::sync::mpsc::UnboundedSender<SseEvent>,
}

impl OpenCodeEvents {
    pub fn new(
        base_url: String,
        password: Option<String>,
        sender: tokio::sync::mpsc::UnboundedSender<SseEvent>,
    ) -> Self {
        OpenCodeEvents {
            base_url,
            password,
            sender,
        }
    }

    /// Spawns the reconnecting SSE listener as a background tokio task.
    ///
    /// The task runs until the CancellationToken is cancelled.
    pub fn start(&self, cancel: CancellationToken) -> tokio::task::JoinHandle<()> {
        let base_url = self.base_url.clone();
        let password = self.password.clone();
        let sender = self.sender.clone();

        tokio::spawn(async move {
            let mut delay_secs: u64 = 1;

            loop {
                if cancel.is_cancelled() {
                    break;
                }

                match connect_and_stream(&base_url, &password, &sender, &cancel).await {
                    Ok(()) => {
                        // Clean disconnect (cancel token fired)
                        break;
                    }
                    Err(e) => {
                        let _ = sender.send(SseEvent::Disconnected(Some(e.to_string())));

                        if cancel.is_cancelled() {
                            break;
                        }

                        // Exponential backoff: 1s, 2s, 4s, ..., 30s max.
                        // Use select! so cancellation wakes us immediately
                        // instead of waiting for the full sleep to elapse.
                        tokio::select! {
                            _ = cancel.cancelled() => break,
                            _ = sleep(Duration::from_secs(delay_secs)) => {}
                        }
                        delay_secs = next_reconnect_delay(delay_secs);
                    }
                }
            }
        })
    }
}

/// Connects to the /event endpoint and streams events until error or cancellation.
async fn connect_and_stream(
    base_url: &str,
    password: &Option<String>,
    sender: &tokio::sync::mpsc::UnboundedSender<SseEvent>,
    cancel: &CancellationToken,
) -> Result<()> {
    let url = format!("{}/event", base_url);
    let client = Client::new();

    let mut req = client
        .get(&url)
        .header("Accept", "text/event-stream")
        .header("Cache-Control", "no-cache");

    if let Some(pw) = password {
        let creds = format!(":{}", pw);
        req = req.header("Authorization", format!("Basic {}", STANDARD.encode(creds)));
    }

    let response = req.send().await?;

    if !response.status().is_success() {
        anyhow::bail!("SSE connection failed with status {}", response.status());
    }

    // Signal successful connection and reset backoff on the caller side
    let _ = sender.send(SseEvent::Connected);

    use futures::StreamExt;
    let mut stream = response.bytes_stream();

    let mut buffer = String::new();

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                return Ok(());
            }
            chunk = stream.next() => {
                match chunk {
                    None => {
                        // Stream ended
                        anyhow::bail!("SSE stream ended unexpectedly");
                    }
                    Some(Err(e)) => {
                        anyhow::bail!("SSE stream error: {}", e);
                    }
                    Some(Ok(bytes)) => {
                        let text = String::from_utf8_lossy(&bytes);
                        buffer.push_str(&text);

                        // Process complete SSE blocks (terminated by \n\n)
                        while let Some(pos) = buffer.find("\n\n") {
                            let block = buffer[..pos].to_string();
                            buffer = buffer[pos + 2..].to_string();
                            if let Some(event) = parse_sse_block(&block) {
                                let _ = sender.send(event);
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Extracts the data field from an SSE block and returns the parsed event.
///
/// Returns `None` for heartbeats, malformed JSON, missing data lines,
/// unknown event types, and other non-actionable blocks.
pub fn parse_sse_block(block: &str) -> Option<SseEvent> {
    // Find "data:" line
    let data = block
        .lines()
        .find(|line| line.starts_with("data:"))
        .map(|line| line.trim_start_matches("data:").trim());

    let data = match data {
        Some(d) if !d.is_empty() => d,
        _ => return None,
    };

    // Parse JSON
    let json: serde_json::Value = match serde_json::from_str(data) {
        Ok(v) => v,
        Err(_) => return None, // Skip malformed JSON
    };

    let event_type = json.get("type").and_then(|v| v.as_str())?;

    let props = json
        .get("properties")
        .cloned()
        .unwrap_or(serde_json::Value::Null);

    match event_type {
        "server.connected" => Some(SseEvent::Connected),
        "server.heartbeat" => None,
        "permission.asked" => {
            serde_json::from_value::<PermissionRequest>(props)
                .ok()
                .map(SseEvent::PermissionAsked)
        }
        "permission.replied" => {
            let session_id = props
                .get("session_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let request_id = props
                .get("request_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let reply = props
                .get("reply")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            Some(SseEvent::PermissionReplied {
                session_id,
                request_id,
                reply,
            })
        }
        "question.asked" => {
            serde_json::from_value::<QuestionRequest>(props)
                .ok()
                .map(SseEvent::QuestionAsked)
        }
        "question.replied" => {
            let session_id = props
                .get("session_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let request_id = props
                .get("request_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let answers = props
                .get("answers")
                .and_then(|v| serde_json::from_value::<Vec<Vec<String>>>(v.clone()).ok())
                .unwrap_or_default();
            Some(SseEvent::QuestionReplied {
                session_id,
                request_id,
                answers,
            })
        }
        "question.rejected" => {
            let session_id = props
                .get("session_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let request_id = props
                .get("request_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            Some(SseEvent::QuestionRejected {
                session_id,
                request_id,
            })
        }
        // Events we intentionally ignore (high-frequency or informational).
        "session.diff"
        | "session.error" | "session.idle" | "session.compacted"
        | "message.updated" | "message.removed" | "message.part.updated"
        | "message.part.delta" | "message.part.removed"
        | "file.edited" | "file.watcher.updated"
        | "project.updated" | "vcs.branch.updated"
        | "todo.updated" | "mcp.tools.changed" | "lsp.updated"
        | "pty.created" | "pty.updated" | "pty.exited" | "pty.deleted"
        | "permission.updated"
        | "installation.updated" | "installation.update-available" => None,
        "session.updated" => {
            let session_id = props
                .get("info")
                .and_then(|v| v.get("id"))
                .or_else(|| props.get("id"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            Some(SseEvent::SessionUpdated { session_id })
        }
        "session.created" => {
            let session_id = props
                .get("info")
                .and_then(|v| v.get("id"))
                .or_else(|| props.get("id"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            Some(SseEvent::SessionCreated { session_id })
        }
        "session.deleted" => {
            let session_id = props
                .get("info")
                .and_then(|v| v.get("id"))
                .or_else(|| props.get("id"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            Some(SseEvent::SessionDeleted { session_id })
        }
        "session.status" => {
            let session_id = props
                .get("sessionID")
                .or_else(|| props.get("session_id"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            // The status field is an object like { "type": "busy" } or { "type": "idle" }.
            let status_type = props
                .get("status")
                .and_then(|v| v.get("type"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let busy = status_type == "busy";
            Some(SseEvent::SessionStatus { session_id, busy })
        }
        other => {
            if is_debug() {
                eprintln!("[voice:debug] Unknown SSE event type: {}", other);
            }
            None
        }
    }
}

/// Returns true when verbose debug logging is enabled via `VOICE_DEBUG=1`.
fn is_debug() -> bool {
    std::env::var("VOICE_DEBUG").map_or(false, |v| v == "1" || v == "true")
}

/// Computes the next reconnect delay using exponential backoff, capped at 30s.
pub fn next_reconnect_delay(current: u64) -> u64 {
    (current * 2).min(30)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_connected() {
        let event = parse_sse_block(
            "data: {\"type\":\"server.connected\",\"properties\":{}}",
        );
        assert!(matches!(event, Some(SseEvent::Connected)));
    }

    #[test]
    fn test_parse_heartbeat_ignored() {
        assert!(parse_sse_block(
            "data: {\"type\":\"server.heartbeat\",\"properties\":{}}"
        ).is_none());
    }

    #[test]
    fn test_parse_malformed_json() {
        assert!(parse_sse_block("data: not-valid-json").is_none());
    }

    #[test]
    fn test_parse_empty_data() {
        assert!(parse_sse_block("event: ping\n").is_none());
    }

    #[test]
    fn test_parse_unknown_type() {
        assert!(parse_sse_block(
            "data: {\"type\":\"unknown.event\",\"properties\":{}}"
        ).is_none());
    }

    #[test]
    fn test_parse_permission_asked() {
        let json = r#"data: {"type":"permission.asked","properties":{"id":"test-id","session_id":"sess","permission":"bash","patterns":[],"metadata":{},"always":[],"tool":null}}"#;
        let event = parse_sse_block(json).unwrap();
        assert!(matches!(event, SseEvent::PermissionAsked(ref req) if req.id == "test-id"));
    }

    #[test]
    fn test_parse_question_asked() {
        let json = r#"data: {"type":"question.asked","properties":{"id":"q1","session_id":"s1","questions":[{"question":"What?","header":"H","options":[],"multiple":false,"custom":true}]}}"#;
        let event = parse_sse_block(json).unwrap();
        assert!(matches!(event, SseEvent::QuestionAsked(ref req) if req.id == "q1"));
    }

    #[test]
    fn test_parse_permission_replied() {
        let json = r#"data: {"type":"permission.replied","properties":{"session_id":"s1","request_id":"r1","reply":"once"}}"#;
        let event = parse_sse_block(json).unwrap();
        assert!(
            matches!(event, SseEvent::PermissionReplied { ref session_id, ref request_id, ref reply }
                if session_id == "s1" && request_id == "r1" && reply == "once")
        );
    }

    #[test]
    fn test_parse_question_replied() {
        let json = r#"data: {"type":"question.replied","properties":{"session_id":"s1","request_id":"r1","answers":[["yes","no"]]}}"#;
        let event = parse_sse_block(json).unwrap();
        assert!(
            matches!(event, SseEvent::QuestionReplied { ref session_id, ref request_id, ref answers }
                if session_id == "s1" && request_id == "r1" && answers == &vec![vec!["yes".to_string(), "no".to_string()]])
        );
    }

    #[test]
    fn test_parse_question_rejected() {
        let json = r#"data: {"type":"question.rejected","properties":{"session_id":"s1","request_id":"r1"}}"#;
        let event = parse_sse_block(json).unwrap();
        assert!(
            matches!(event, SseEvent::QuestionRejected { ref session_id, ref request_id }
                if session_id == "s1" && request_id == "r1")
        );
    }

    #[test]
    fn test_backoff_calculation() {
        let mut delay: u64 = 1;
        let sequence: Vec<u64> = (0..8)
            .map(|_| {
                let d = delay;
                delay = next_reconnect_delay(delay);
                d
            })
            .collect();
        assert_eq!(sequence, vec![1, 2, 4, 8, 16, 30, 30, 30]);
    }

    // ── session.status ────────────────────────────────────────────────

    #[test]
    fn test_parse_session_status_busy() {
        let json = r#"data: {"type":"session.status","properties":{"sessionID":"s1","status":{"type":"busy"}}}"#;
        let event = parse_sse_block(json).unwrap();
        assert!(
            matches!(event, SseEvent::SessionStatus { ref session_id, busy }
                if session_id == "s1" && busy)
        );
    }

    #[test]
    fn test_parse_session_status_idle() {
        let json = r#"data: {"type":"session.status","properties":{"sessionID":"s1","status":{"type":"idle"}}}"#;
        let event = parse_sse_block(json).unwrap();
        assert!(
            matches!(event, SseEvent::SessionStatus { ref session_id, busy }
                if session_id == "s1" && !busy)
        );
    }

    #[test]
    fn test_parse_session_status_retry() {
        // "retry" is neither "busy" — should parse as not busy.
        let json = r#"data: {"type":"session.status","properties":{"sessionID":"s1","status":{"type":"retry"}}}"#;
        let event = parse_sse_block(json).unwrap();
        assert!(
            matches!(event, SseEvent::SessionStatus { busy, .. } if !busy)
        );
    }

    #[test]
    fn test_parse_session_status_missing_status_field() {
        // Missing status object → defaults to not busy.
        let json = r#"data: {"type":"session.status","properties":{"sessionID":"s1"}}"#;
        let event = parse_sse_block(json).unwrap();
        assert!(
            matches!(event, SseEvent::SessionStatus { busy, .. } if !busy)
        );
    }

    #[test]
    fn test_parse_session_status_snake_case_session_id() {
        // Uses "session_id" instead of "sessionID" — both should be supported.
        let json = r#"data: {"type":"session.status","properties":{"session_id":"s2","status":{"type":"busy"}}}"#;
        let event = parse_sse_block(json).unwrap();
        assert!(
            matches!(event, SseEvent::SessionStatus { ref session_id, busy }
                if session_id == "s2" && busy)
        );
    }

    // ── session lifecycle events ──────────────────────────────────────

    #[test]
    fn test_parse_session_updated() {
        let json = r#"data: {"type":"session.updated","properties":{"info":{"id":"sess_1"}}}"#;
        let event = parse_sse_block(json).unwrap();
        assert!(
            matches!(event, SseEvent::SessionUpdated { ref session_id } if session_id == "sess_1"),
            "SessionUpdated session_id should be 'sess_1'"
        );
    }

    #[test]
    fn test_parse_session_created() {
        let json = r#"data: {"type":"session.created","properties":{"info":{"id":"sess_2"}}}"#;
        let event = parse_sse_block(json).unwrap();
        assert!(
            matches!(event, SseEvent::SessionCreated { ref session_id } if session_id == "sess_2"),
            "SessionCreated session_id should be 'sess_2'"
        );
    }

    #[test]
    fn test_parse_session_deleted() {
        let json = r#"data: {"type":"session.deleted","properties":{"info":{"id":"sess_3"}}}"#;
        let event = parse_sse_block(json).unwrap();
        assert!(
            matches!(event, SseEvent::SessionDeleted { ref session_id } if session_id == "sess_3"),
            "SessionDeleted session_id should be 'sess_3'"
        );
    }

    #[test]
    fn test_parse_session_updated_direct_id() {
        // Some server versions may send id directly in properties rather than info.id
        let json = r#"data: {"type":"session.updated","properties":{"id":"sess_direct"}}"#;
        let event = parse_sse_block(json).unwrap();
        assert!(
            matches!(event, SseEvent::SessionUpdated { ref session_id } if session_id == "sess_direct"),
            "SessionUpdated should fall back to direct id field"
        );
    }

    // ── explicitly ignored events ─────────────────────────────────────

    #[test]
    fn test_parse_ignored_session_events_return_none() {
        for event_type in &[
            "session.diff",
            "session.error",
            "session.idle",
            "session.compacted",
        ] {
            let json = format!(
                r#"data: {{"type":"{}","properties":{{}}}}"#,
                event_type
            );
            assert!(
                parse_sse_block(&json).is_none(),
                "{} should be explicitly ignored",
                event_type
            );
        }
    }

    #[test]
    fn test_parse_ignored_message_events_return_none() {
        for event_type in &[
            "message.updated",
            "message.removed",
            "message.part.updated",
            "message.part.delta",
            "message.part.removed",
        ] {
            let json = format!(
                r#"data: {{"type":"{}","properties":{{}}}}"#,
                event_type
            );
            assert!(
                parse_sse_block(&json).is_none(),
                "{} should be explicitly ignored",
                event_type
            );
        }
    }

    #[test]
    fn test_parse_ignored_misc_events_return_none() {
        for event_type in &[
            "file.edited",
            "file.watcher.updated",
            "project.updated",
            "vcs.branch.updated",
            "todo.updated",
            "mcp.tools.changed",
            "lsp.updated",
            "pty.created",
            "pty.updated",
            "pty.exited",
            "pty.deleted",
            "permission.updated",
            "installation.updated",
            "installation.update-available",
        ] {
            let json = format!(
                r#"data: {{"type":"{}","properties":{{}}}}"#,
                event_type
            );
            assert!(
                parse_sse_block(&json).is_none(),
                "{} should be explicitly ignored",
                event_type
            );
        }
    }

    // ── truly unknown events ──────────────────────────────────────────

    #[test]
    fn test_parse_truly_unknown_event_returns_none() {
        // An event type not in the ignore list and not handled → still returns None.
        assert!(parse_sse_block(
            r#"data: {"type":"some.future.event","properties":{}}"#
        ).is_none());
    }

    #[test]
    fn test_parse_no_type_field() {
        assert!(parse_sse_block("data: {\"properties\":{}}").is_none());
    }

    #[test]
    fn test_parse_missing_properties() {
        let event = parse_sse_block("data: {\"type\":\"server.connected\"}");
        assert!(matches!(event, Some(SseEvent::Connected)));
    }
}
