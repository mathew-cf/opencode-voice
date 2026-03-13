//! Integration tests for OpenCodeBridge HTTP client.
//!
//! Each test spins up a minimal tokio TCP listener on port 0 (OS-assigned),
//! sends a canned HTTP response, and verifies that the bridge client sends
//! the correct request.

use base64::{engine::general_purpose::STANDARD, Engine as _};
use opencode_voice::bridge::client::OpenCodeBridge;
use opencode_voice::bridge::client::SessionInfo;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Mutex;

/// Binds a TCP listener on port 0 and returns it together with the assigned port.
async fn bind_listener() -> (TcpListener, u16) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    (listener, port)
}

/// Reads the full HTTP request from the socket (up to 8 KB) and returns it as a String.
async fn read_request(stream: &mut tokio::net::TcpStream) -> String {
    let mut buf = vec![0u8; 8192];
    let n = stream.read(&mut buf).await.unwrap_or(0);
    String::from_utf8_lossy(&buf[..n]).to_string()
}

/// Sends a minimal HTTP 200 OK response with an optional JSON body.
async fn send_ok(stream: &mut tokio::net::TcpStream, body: &str) {
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    stream.write_all(response.as_bytes()).await.unwrap();
}

/// Sends a minimal HTTP 204 No Content response.
async fn send_no_content(stream: &mut tokio::net::TcpStream) {
    let response = "HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n";
    stream.write_all(response.as_bytes()).await.unwrap();
}

// ─── Test 1: get_base_url() returns correct values ───────────────────────────

#[test]
fn test_get_base_url() {
    let bridge = OpenCodeBridge::new("http://localhost", 4096, None);
    assert_eq!(bridge.get_base_url(), "http://localhost:4096");
}

#[test]
fn test_get_base_url_trailing_slash_stripped() {
    let bridge = OpenCodeBridge::new("http://localhost/", 1234, None);
    assert_eq!(bridge.get_base_url(), "http://localhost:1234");
}

// ─── Test 2: is_connected() returns false when server is not running ──────────

#[tokio::test]
async fn test_is_connected_returns_false_when_server_not_running() {
    // Port 1 is almost certainly not listening; connection should be refused.
    // We use a port that we know is not bound by binding and immediately dropping.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener); // Release the port — nothing is listening now.

    let bridge = OpenCodeBridge::new("http://127.0.0.1", port, None);
    assert!(!bridge.is_connected().await);
}

// ─── Test 3: auth header is correct when password is set ─────────────────────

#[tokio::test]
async fn test_auth_header_sent_when_password_set() {
    let (listener, port) = bind_listener().await;
    let captured = Arc::new(Mutex::new(String::new()));
    let captured_clone = captured.clone();

    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let req = read_request(&mut stream).await;
        *captured_clone.lock().await = req;
        send_no_content(&mut stream).await;
    });

    let password = "s3cr3t";
    let bridge = OpenCodeBridge::new("http://127.0.0.1", port, Some(password.to_string()));
    bridge
        .send_message("sess_test", "test message")
        .await
        .expect("send_message should succeed");

    let req = captured.lock().await.clone();

    // Expected: Basic base64("opencode:s3cr3t") — username is "opencode"
    // reqwest sends headers in lowercase (HTTP/2 style), so check case-insensitively.
    let expected_creds = STANDARD.encode(format!("opencode:{}", password));
    let expected_value = format!("Basic {}", expected_creds);
    let req_lower = req.to_lowercase();
    let expected_lower = format!("authorization: {}", expected_value.to_lowercase());

    assert!(
        req_lower.contains(&expected_lower),
        "Request should contain auth header 'authorization: {}', got:\n{}",
        expected_value,
        req
    );
}

// ─── Test 4: no auth header when no password ─────────────────────────────────

#[tokio::test]
async fn test_no_auth_header_when_no_password() {
    let (listener, port) = bind_listener().await;
    let captured = Arc::new(Mutex::new(String::new()));
    let captured_clone = captured.clone();

    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let req = read_request(&mut stream).await;
        *captured_clone.lock().await = req;
        send_no_content(&mut stream).await;
    });

    let bridge = OpenCodeBridge::new("http://127.0.0.1", port, None);
    bridge
        .send_message("sess_test", "test message")
        .await
        .expect("send_message should succeed");

    let req = captured.lock().await.clone();
    assert!(
        !req.contains("Authorization:"),
        "Request should NOT contain Authorization header when no password is set"
    );
}

// ─── Test 5: reply_permission sends POST to /permission/{id}/reply ────────────

#[tokio::test]
async fn test_reply_permission_sends_correct_request() {
    use opencode_voice::approval::types::PermissionReply;

    let (listener, port) = bind_listener().await;
    let captured = Arc::new(Mutex::new(String::new()));
    let captured_clone = captured.clone();

    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let req = read_request(&mut stream).await;
        *captured_clone.lock().await = req;
        send_ok(&mut stream, "{}").await;
    });

    let bridge = OpenCodeBridge::new("http://127.0.0.1", port, None);
    bridge
        .reply_permission("perm-123", PermissionReply::Once, None)
        .await
        .expect("reply_permission should succeed");

    let req = captured.lock().await.clone();

    assert!(
        req.starts_with("POST /permission/perm-123/reply"),
        "Expected POST /permission/perm-123/reply, got: {}",
        req.lines().next().unwrap_or("")
    );

    // Body should contain the reply field
    assert!(
        req.contains(r#""reply":"once""#) || req.contains(r#""reply": "once""#),
        "Request body should contain reply field: {}",
        req
    );
}

// ─── Test 6: reject_question sends POST to /question/{id}/reject ──────────────

#[tokio::test]
async fn test_reject_question_sends_correct_request() {
    let (listener, port) = bind_listener().await;
    let captured = Arc::new(Mutex::new(String::new()));
    let captured_clone = captured.clone();

    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let req = read_request(&mut stream).await;
        *captured_clone.lock().await = req;
        send_ok(&mut stream, "{}").await;
    });

    let bridge = OpenCodeBridge::new("http://127.0.0.1", port, None);
    bridge
        .reject_question("q-456")
        .await
        .expect("reject_question should succeed");

    let req = captured.lock().await.clone();

    assert!(
        req.starts_with("POST /question/q-456/reject"),
        "Expected POST /question/q-456/reject, got: {}",
        req.lines().next().unwrap_or("")
    );
}

// ─── Test 7: is_connected() returns true when server responds ─────────────────

#[tokio::test]
async fn test_is_connected_returns_true_when_server_running() {
    let (listener, port) = bind_listener().await;
    let captured = Arc::new(Mutex::new(String::new()));
    let captured_clone = captured.clone();

    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let req = read_request(&mut stream).await;
        *captured_clone.lock().await = req;
        send_ok(&mut stream, r#"{"healthy":true,"version":"1.0"}"#).await;
    });

    let bridge = OpenCodeBridge::new("http://127.0.0.1", port, None);
    assert!(bridge.is_connected().await);

    let req = captured.lock().await.clone();
    assert!(
        req.contains("GET /global/health"),
        "is_connected should hit /global/health, got: {}",
        req.lines().next().unwrap_or("")
    );
}

// ─── Test 8: reply_permission with Always reply and message ──────────────────

#[tokio::test]
async fn test_reply_permission_always_with_message() {
    use opencode_voice::approval::types::PermissionReply;

    let (listener, port) = bind_listener().await;
    let captured = Arc::new(Mutex::new(String::new()));
    let captured_clone = captured.clone();

    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let req = read_request(&mut stream).await;
        *captured_clone.lock().await = req;
        send_ok(&mut stream, "{}").await;
    });

    let bridge = OpenCodeBridge::new("http://127.0.0.1", port, None);
    bridge
        .reply_permission("perm-789", PermissionReply::Always, Some("approved by voice"))
        .await
        .expect("reply_permission should succeed");

    let req = captured.lock().await.clone();

    assert!(
        req.contains(r#""reply":"always""#) || req.contains(r#""reply": "always""#),
        "Body should contain always reply: {}",
        req
    );
    assert!(
        req.contains("approved by voice"),
        "Body should contain the message: {}",
        req
    );
}

// ─── Test 9: health_check returns true when server is healthy ─────────────────

#[tokio::test]
async fn test_health_check_returns_true_when_healthy() {
    let (listener, port) = bind_listener().await;

    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 4096];
        let _ = stream.read(&mut buf).await;
        send_ok(&mut stream, r#"{"healthy":true,"version":"1.0"}"#).await;
    });

    let bridge = OpenCodeBridge::new("http://127.0.0.1", port, None);
    let result = bridge.health_check().await;
    assert!(result.is_ok());
    assert!(result.unwrap(), "health_check should return true when healthy=true");
}

// ─── Test 10: health_check returns false on server error ──────────────────────

#[tokio::test]
async fn test_health_check_returns_false_on_server_error() {
    let (listener, port) = bind_listener().await;

    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 4096];
        let _ = stream.read(&mut buf).await;
        let response = "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}";
        stream.write_all(response.as_bytes()).await.unwrap();
    });

    let bridge = OpenCodeBridge::new("http://127.0.0.1", port, None);
    let result = bridge.health_check().await;
    assert!(result.is_ok(), "health_check should not error on 500, got: {:?}", result);
    assert!(!result.unwrap(), "health_check should return false on 500");
}

// ─── Test 11: list_sessions parses response correctly ─────────────────────────

#[tokio::test]
async fn test_list_sessions_parses_response() {
    let (listener, port) = bind_listener().await;
    let captured = Arc::new(Mutex::new(String::new()));
    let captured_clone = captured.clone();

    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let req = read_request(&mut stream).await;
        *captured_clone.lock().await = req;
        send_ok(&mut stream, r#"[{"id":"sess_1","title":"My Session","time_updated":1234567890}]"#).await;
    });

    let bridge = OpenCodeBridge::new("http://127.0.0.1", port, None);
    let sessions: Vec<SessionInfo> = bridge.list_sessions().await.expect("list_sessions should succeed");

    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, "sess_1");
    assert_eq!(sessions[0].title, "My Session");

    // Verify the request hits the correct path
    let req = captured.lock().await.clone();
    assert!(
        req.contains("GET /session"),
        "list_sessions should hit /session, got: {}",
        req.lines().next().unwrap_or("")
    );
}

// ─── Test 12: create_session sends POST with empty body ───────────────────────

#[tokio::test]
async fn test_create_session_sends_empty_body() {
    let (listener, port) = bind_listener().await;
    let captured = Arc::new(Mutex::new(String::new()));
    let captured_clone = captured.clone();

    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let req = read_request(&mut stream).await;
        *captured_clone.lock().await = req;
        send_ok(&mut stream, r#"{"id":"new_sess","title":"New Session"}"#).await;
    });

    let bridge = OpenCodeBridge::new("http://127.0.0.1", port, None);
    let session = bridge.create_session().await.expect("create_session should succeed");

    assert_eq!(session.id, "new_sess");
    assert_eq!(session.title, "New Session");

    let req = captured.lock().await.clone();
    assert!(
        req.starts_with("POST /session"),
        "create_session should POST to /session, got: {}",
        req.lines().next().unwrap_or("")
    );
}

// ─── Test 13: send_message sends correct request ──────────────────────────────

#[tokio::test]
async fn test_send_message_sends_correct_request() {
    let (listener, port) = bind_listener().await;
    let captured = Arc::new(Mutex::new(String::new()));
    let captured_clone = captured.clone();

    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let req = read_request(&mut stream).await;
        *captured_clone.lock().await = req;
        send_no_content(&mut stream).await;
    });

    let bridge = OpenCodeBridge::new("http://127.0.0.1", port, None);
    bridge
        .send_message("sess_abc", "hello world")
        .await
        .expect("send_message should succeed");

    let req = captured.lock().await.clone();
    let first_line = req.lines().next().unwrap_or("");

    assert!(
        first_line.contains("POST /session/sess_abc/prompt_async"),
        "send_message should POST to /session/sess_abc/prompt_async, got: {}",
        first_line
    );

    // Body should contain the parts array with type=text
    assert!(
        req.contains(r#""type":"text""#) || req.contains(r#""type": "text""#),
        "Body should contain type=text in parts: {}",
        req
    );
    assert!(
        req.contains("hello world"),
        "Body should contain the message text: {}",
        req
    );
}
