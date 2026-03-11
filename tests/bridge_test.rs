//! Integration tests for OpenCodeBridge HTTP client.
//!
//! Each test spins up a minimal tokio TCP listener on port 0 (OS-assigned),
//! sends a canned HTTP response, and verifies that the bridge client sends
//! the correct request.

use base64::{engine::general_purpose::STANDARD, Engine as _};
use opencode_voice::bridge::client::OpenCodeBridge;
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

// ─── Test 3: append_prompt sends POST to /tui/append-prompt with correct JSON ─

#[tokio::test]
async fn test_append_prompt_sends_correct_request() {
    let (listener, port) = bind_listener().await;
    let captured = Arc::new(Mutex::new(String::new()));
    let captured_clone = captured.clone();

    // Spawn a task that accepts one connection, captures the request, and responds 200.
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let req = read_request(&mut stream).await;
        *captured_clone.lock().await = req;
        send_ok(&mut stream, "{}").await;
    });

    let bridge = OpenCodeBridge::new("http://127.0.0.1", port, None);
    bridge
        .append_prompt("hello", None, None)
        .await
        .expect("append_prompt should succeed");

    let req = captured.lock().await.clone();

    // Verify the request line
    assert!(
        req.starts_with("POST /tui/append-prompt"),
        "Expected POST /tui/append-prompt, got: {}",
        req.lines().next().unwrap_or("")
    );

    // Verify the JSON body contains the text field
    assert!(
        req.contains(r#""text":"hello""#) || req.contains(r#""text": "hello""#),
        "Request body should contain text field: {}",
        req
    );
}

// ─── Test 4: auth header is correct when password is set ─────────────────────

#[tokio::test]
async fn test_auth_header_sent_when_password_set() {
    let (listener, port) = bind_listener().await;
    let captured = Arc::new(Mutex::new(String::new()));
    let captured_clone = captured.clone();

    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let req = read_request(&mut stream).await;
        *captured_clone.lock().await = req;
        send_ok(&mut stream, "{}").await;
    });

    let password = "s3cr3t";
    let bridge = OpenCodeBridge::new("http://127.0.0.1", port, Some(password.to_string()));
    bridge
        .append_prompt("test", None, None)
        .await
        .expect("append_prompt should succeed");

    let req = captured.lock().await.clone();

    // Expected: Basic base64(":s3cr3t")
    // reqwest sends headers in lowercase (HTTP/2 style), so check case-insensitively.
    let expected_creds = STANDARD.encode(format!(":{}", password));
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

// ─── Test 5: no auth header when no password ─────────────────────────────────

#[tokio::test]
async fn test_no_auth_header_when_no_password() {
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
        .append_prompt("test", None, None)
        .await
        .expect("append_prompt should succeed");

    let req = captured.lock().await.clone();
    assert!(
        !req.contains("Authorization:"),
        "Request should NOT contain Authorization header when no password is set"
    );
}

// ─── Test 6: reply_permission sends POST to /permission/{id}/reply ────────────

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

// ─── Test 7: reject_question sends POST to /question/{id}/reject ──────────────

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

// ─── Test 8: is_connected() returns true when server responds ─────────────────

#[tokio::test]
async fn test_is_connected_returns_true_when_server_running() {
    let (listener, port) = bind_listener().await;

    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        // Consume the request
        let mut buf = vec![0u8; 4096];
        let _ = stream.read(&mut buf).await;
        send_ok(&mut stream, "{}").await;
    });

    let bridge = OpenCodeBridge::new("http://127.0.0.1", port, None);
    assert!(bridge.is_connected().await);
}

// ─── Test 9: append_prompt with directory and workspace query params ──────────

#[tokio::test]
async fn test_append_prompt_with_query_params() {
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
        .append_prompt("hello", Some("/home/user"), Some("/workspace"))
        .await
        .expect("append_prompt should succeed");

    let req = captured.lock().await.clone();
    let first_line = req.lines().next().unwrap_or("");

    assert!(
        first_line.contains("directory="),
        "Request URL should contain directory param: {}",
        first_line
    );
    assert!(
        first_line.contains("workspace="),
        "Request URL should contain workspace param: {}",
        first_line
    );
}

// ─── Test 10: reply_permission with Always reply and message ──────────────────

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
