//! Integration tests for OpenCodeBridge HTTP client.
//!
//! Each test spins up a wiremock MockServer, mounts the expected route,
//! and verifies that the bridge client sends the correct request.

use base64::{engine::general_purpose::STANDARD, Engine as _};
use opencode_voice::bridge::client::OpenCodeBridge;
use opencode_voice::bridge::client::SessionInfo;
use tokio::net::TcpListener;
use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

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
    let mock_server = MockServer::start().await;
    let port = mock_server.address().port();

    Mock::given(method("POST"))
        .and(path_regex(r"^/session/.+/prompt_async$"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&mock_server)
        .await;

    let password = "s3cr3t";
    let bridge = OpenCodeBridge::new("http://127.0.0.1", port, Some(password.to_string()));
    bridge
        .send_message("sess_test", "test message")
        .await
        .expect("send_message should succeed");

    let requests = mock_server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);

    // Expected: Basic base64("opencode:s3cr3t") — username is "opencode"
    let expected_creds = STANDARD.encode(format!("opencode:{}", password));
    let expected_value = format!("Basic {}", expected_creds);

    let auth_header = requests[0]
        .headers
        .get("authorization")
        .expect("Authorization header should be present");

    assert_eq!(
        auth_header.to_str().unwrap(),
        expected_value,
        "Authorization header should be 'Basic <creds>'"
    );
}

// ─── Test 4: no auth header when no password ─────────────────────────────────

#[tokio::test]
async fn test_no_auth_header_when_no_password() {
    let mock_server = MockServer::start().await;
    let port = mock_server.address().port();

    Mock::given(method("POST"))
        .and(path_regex(r"^/session/.+/prompt_async$"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&mock_server)
        .await;

    let bridge = OpenCodeBridge::new("http://127.0.0.1", port, None);
    bridge
        .send_message("sess_test", "test message")
        .await
        .expect("send_message should succeed");

    let requests = mock_server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);

    assert!(
        requests[0].headers.get("authorization").is_none(),
        "Request should NOT contain Authorization header when no password is set"
    );
}

// ─── Test 5: reply_permission sends POST to /permission/{id}/reply ────────────

#[tokio::test]
async fn test_reply_permission_sends_correct_request() {
    use opencode_voice::approval::types::PermissionReply;

    let mock_server = MockServer::start().await;
    let port = mock_server.address().port();

    Mock::given(method("POST"))
        .and(path("/permission/perm-123/reply"))
        .respond_with(ResponseTemplate::new(200).set_body_string("{}"))
        .mount(&mock_server)
        .await;

    let bridge = OpenCodeBridge::new("http://127.0.0.1", port, None);
    bridge
        .reply_permission("perm-123", PermissionReply::Once, None)
        .await
        .expect("reply_permission should succeed");

    let requests = mock_server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].url.path(), "/permission/perm-123/reply");

    // Body should contain the reply field
    let body = String::from_utf8_lossy(&requests[0].body);
    assert!(
        body.contains(r#""reply":"once""#) || body.contains(r#""reply": "once""#),
        "Request body should contain reply field: {}",
        body
    );
}

// ─── Test 6: reject_question sends POST to /question/{id}/reject ──────────────

#[tokio::test]
async fn test_reject_question_sends_correct_request() {
    let mock_server = MockServer::start().await;
    let port = mock_server.address().port();

    Mock::given(method("POST"))
        .and(path("/question/q-456/reject"))
        .respond_with(ResponseTemplate::new(200).set_body_string("{}"))
        .mount(&mock_server)
        .await;

    let bridge = OpenCodeBridge::new("http://127.0.0.1", port, None);
    bridge
        .reject_question("q-456")
        .await
        .expect("reject_question should succeed");

    let requests = mock_server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].url.path(),
        "/question/q-456/reject",
        "Expected POST /question/q-456/reject"
    );
}

// ─── Test 7: is_connected() returns true when server responds ─────────────────

#[tokio::test]
async fn test_is_connected_returns_true_when_server_running() {
    let mock_server = MockServer::start().await;
    let port = mock_server.address().port();

    Mock::given(method("GET"))
        .and(path("/global/health"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(r#"{"healthy":true,"version":"1.0"}"#),
        )
        .mount(&mock_server)
        .await;

    let bridge = OpenCodeBridge::new("http://127.0.0.1", port, None);
    assert!(bridge.is_connected().await);

    let requests = mock_server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].url.path(),
        "/global/health",
        "is_connected should hit /global/health"
    );
}

// ─── Test 8: reply_permission with Always reply and message ──────────────────

#[tokio::test]
async fn test_reply_permission_always_with_message() {
    use opencode_voice::approval::types::PermissionReply;

    let mock_server = MockServer::start().await;
    let port = mock_server.address().port();

    Mock::given(method("POST"))
        .and(path("/permission/perm-789/reply"))
        .respond_with(ResponseTemplate::new(200).set_body_string("{}"))
        .mount(&mock_server)
        .await;

    let bridge = OpenCodeBridge::new("http://127.0.0.1", port, None);
    bridge
        .reply_permission("perm-789", PermissionReply::Always, Some("approved by voice"))
        .await
        .expect("reply_permission should succeed");

    let requests = mock_server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);

    let body = String::from_utf8_lossy(&requests[0].body);
    assert!(
        body.contains(r#""reply":"always""#) || body.contains(r#""reply": "always""#),
        "Body should contain always reply: {}",
        body
    );
    assert!(
        body.contains("approved by voice"),
        "Body should contain the message: {}",
        body
    );
}

// ─── Test 9: health_check returns true when server is healthy ─────────────────

#[tokio::test]
async fn test_health_check_returns_true_when_healthy() {
    let mock_server = MockServer::start().await;
    let port = mock_server.address().port();

    Mock::given(method("GET"))
        .and(path("/global/health"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(r#"{"healthy":true,"version":"1.0"}"#),
        )
        .mount(&mock_server)
        .await;

    let bridge = OpenCodeBridge::new("http://127.0.0.1", port, None);
    let result = bridge.health_check().await;
    assert!(result.is_ok());
    assert!(result.unwrap(), "health_check should return true when healthy=true");
}

// ─── Test 10: health_check returns false on server error ──────────────────────

#[tokio::test]
async fn test_health_check_returns_false_on_server_error() {
    let mock_server = MockServer::start().await;
    let port = mock_server.address().port();

    Mock::given(method("GET"))
        .and(path("/global/health"))
        .respond_with(ResponseTemplate::new(500).set_body_string("{}"))
        .mount(&mock_server)
        .await;

    let bridge = OpenCodeBridge::new("http://127.0.0.1", port, None);
    let result = bridge.health_check().await;
    assert!(result.is_ok(), "health_check should not error on 500, got: {:?}", result);
    assert!(!result.unwrap(), "health_check should return false on 500");
}

// ─── Test 11: list_sessions parses response correctly ─────────────────────────

#[tokio::test]
async fn test_list_sessions_parses_response() {
    let mock_server = MockServer::start().await;
    let port = mock_server.address().port();

    // wiremock's path() matcher matches the path portion only (ignores query strings)
    Mock::given(method("GET"))
        .and(path("/session"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(
                r#"[{"id":"sess_1","title":"My Session","time_updated":1234567890}]"#,
            ),
        )
        .mount(&mock_server)
        .await;

    let bridge = OpenCodeBridge::new("http://127.0.0.1", port, None);
    let sessions: Vec<SessionInfo> = bridge.list_sessions().await.expect("list_sessions should succeed");

    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, "sess_1");
    assert_eq!(sessions[0].title, "My Session");

    // Verify the request hits the correct path
    let requests = mock_server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].url.path(),
        "/session",
        "list_sessions should hit /session"
    );
}

// ─── Test 12: create_session sends POST with empty body ───────────────────────

#[tokio::test]
async fn test_create_session_sends_empty_body() {
    let mock_server = MockServer::start().await;
    let port = mock_server.address().port();

    Mock::given(method("POST"))
        .and(path("/session"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(r#"{"id":"new_sess","title":"New Session"}"#),
        )
        .mount(&mock_server)
        .await;

    let bridge = OpenCodeBridge::new("http://127.0.0.1", port, None);
    let session = bridge.create_session().await.expect("create_session should succeed");

    assert_eq!(session.id, "new_sess");
    assert_eq!(session.title, "New Session");

    let requests = mock_server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].url.path(),
        "/session",
        "create_session should POST to /session"
    );
    assert_eq!(requests[0].method.as_str(), "POST");
}

// ─── Test 13: send_message sends correct request ──────────────────────────────

#[tokio::test]
async fn test_send_message_sends_correct_request() {
    let mock_server = MockServer::start().await;
    let port = mock_server.address().port();

    Mock::given(method("POST"))
        .and(path("/session/sess_abc/prompt_async"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&mock_server)
        .await;

    let bridge = OpenCodeBridge::new("http://127.0.0.1", port, None);
    bridge
        .send_message("sess_abc", "hello world")
        .await
        .expect("send_message should succeed");

    let requests = mock_server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].url.path(), "/session/sess_abc/prompt_async");

    let body = String::from_utf8_lossy(&requests[0].body);

    // Body should contain the parts array with type=text
    assert!(
        body.contains(r#""type":"text""#) || body.contains(r#""type": "text""#),
        "Body should contain type=text in parts: {}",
        body
    );
    assert!(
        body.contains("hello world"),
        "Body should contain the message text: {}",
        body
    );
}
