//! Integration tests for SSE event parsing and reconnect backoff logic.
//!
//! These tests exercise `parse_sse_block` (a public wrapper around the internal
//! `process_sse_block`) and `next_reconnect_delay` (the backoff helper), both
//! exposed from `src/bridge/events.rs`.

use opencode_voice::bridge::events::{next_reconnect_delay, parse_sse_block, SseEvent};

// ─── SSE parsing tests ────────────────────────────────────────────────────────

/// `server.heartbeat` events must be silently ignored (return None).
#[test]
fn test_heartbeat_produces_no_event() {
    let block = r#"data: {"type":"server.heartbeat","properties":{}}"#;
    assert!(
        parse_sse_block(block).is_none(),
        "server.heartbeat should produce no event"
    );
}

/// Malformed JSON in the data line must be skipped (return None).
#[test]
fn test_malformed_json_is_skipped() {
    let block = "data: this-is-not-json";
    assert!(
        parse_sse_block(block).is_none(),
        "Malformed JSON should be skipped"
    );
}

/// An SSE block with no `data:` line must be skipped (return None).
#[test]
fn test_empty_data_line_is_skipped() {
    let block = "event: ping\nid: 1";
    assert!(
        parse_sse_block(block).is_none(),
        "Block with no data: line should be skipped"
    );
}

/// An SSE block with an empty data value must be skipped (return None).
#[test]
fn test_blank_data_value_is_skipped() {
    let block = "data: ";
    assert!(
        parse_sse_block(block).is_none(),
        "Block with blank data value should be skipped"
    );
}

/// `permission.asked` event must parse into `SseEvent::PermissionAsked` with
/// the correct `id` field.
#[test]
fn test_permission_asked_parses_correctly() {
    let block = r#"data: {"type":"permission.asked","properties":{"id":"perm-001","session_id":"sess-1","permission":"bash","patterns":[],"metadata":{},"always":[],"tool":null}}"#;
    let event = parse_sse_block(block).expect("permission.asked should produce an event");
    match event {
        SseEvent::PermissionAsked(req) => {
            assert_eq!(req.id, "perm-001");
            assert_eq!(req.permission, "bash");
        }
        other => panic!("Expected PermissionAsked, got {:?}", other),
    }
}

/// `permission.replied` event must parse into `SseEvent::PermissionReplied`
/// with the correct fields.
#[test]
fn test_permission_replied_parses_correctly() {
    let block = r#"data: {"type":"permission.replied","properties":{"session_id":"s1","request_id":"r1","reply":"once"}}"#;
    let event = parse_sse_block(block).expect("permission.replied should produce an event");
    match event {
        SseEvent::PermissionReplied {
            session_id,
            request_id,
            reply,
        } => {
            assert_eq!(session_id, "s1");
            assert_eq!(request_id, "r1");
            assert_eq!(reply, "once");
        }
        other => panic!("Expected PermissionReplied, got {:?}", other),
    }
}

/// `question.asked` event must parse into `SseEvent::QuestionAsked` with the
/// correct `id` and `session_id`.
#[test]
fn test_question_asked_parses_correctly() {
    let block = r#"data: {"type":"question.asked","properties":{"id":"q-42","session_id":"sess-2","questions":[{"question":"Continue?","header":"Confirm","options":[],"multiple":false,"custom":true}],"tool":null}}"#;
    let event = parse_sse_block(block).expect("question.asked should produce an event");
    match event {
        SseEvent::QuestionAsked(req) => {
            assert_eq!(req.id, "q-42");
            assert_eq!(req.questions.len(), 1);
            assert_eq!(req.questions[0].question, "Continue?");
        }
        other => panic!("Expected QuestionAsked, got {:?}", other),
    }
}

/// `question.replied` event must parse into `SseEvent::QuestionReplied` with
/// the correct answers.
#[test]
fn test_question_replied_parses_correctly() {
    let block = r#"data: {"type":"question.replied","properties":{"session_id":"s2","request_id":"r2","answers":[["yes","no"],["maybe"]]}}"#;
    let event = parse_sse_block(block).expect("question.replied should produce an event");
    match event {
        SseEvent::QuestionReplied {
            session_id,
            request_id,
            answers,
        } => {
            assert_eq!(session_id, "s2");
            assert_eq!(request_id, "r2");
            assert_eq!(answers, vec![vec!["yes", "no"], vec!["maybe"]]);
        }
        other => panic!("Expected QuestionReplied, got {:?}", other),
    }
}

/// `question.rejected` event must parse into `SseEvent::QuestionRejected`.
#[test]
fn test_question_rejected_parses_correctly() {
    let block =
        r#"data: {"type":"question.rejected","properties":{"session_id":"s3","request_id":"r3"}}"#;
    let event = parse_sse_block(block).expect("question.rejected should produce an event");
    match event {
        SseEvent::QuestionRejected {
            session_id,
            request_id,
        } => {
            assert_eq!(session_id, "s3");
            assert_eq!(request_id, "r3");
        }
        other => panic!("Expected QuestionRejected, got {:?}", other),
    }
}

/// `server.connected` event must parse into `SseEvent::Connected`.
#[test]
fn test_server_connected_parses_correctly() {
    let block = r#"data: {"type":"server.connected","properties":{}}"#;
    let event = parse_sse_block(block).expect("server.connected should produce an event");
    assert!(
        matches!(event, SseEvent::Connected),
        "Expected Connected, got {:?}",
        event
    );
}

/// Unknown event types must be silently ignored (return None).
#[test]
fn test_unknown_event_type_is_ignored() {
    let block = r#"data: {"type":"some.future.event","properties":{"foo":"bar"}}"#;
    assert!(
        parse_sse_block(block).is_none(),
        "Unknown event type should be silently ignored"
    );
}

/// JSON without a `type` field must be silently ignored (return None).
#[test]
fn test_missing_type_field_is_ignored() {
    let block = r#"data: {"properties":{"id":"x"}}"#;
    assert!(
        parse_sse_block(block).is_none(),
        "JSON without type field should be ignored"
    );
}

/// Multi-line SSE block: the `data:` line is found even when other lines
/// (e.g. `event:`, `id:`) precede it.
#[test]
fn test_multiline_sse_block_finds_data_line() {
    let block = "event: message\nid: 99\ndata: {\"type\":\"server.connected\"}";
    let event = parse_sse_block(block).expect("should find data: line in multi-line block");
    assert!(matches!(event, SseEvent::Connected));
}

// ─── Session lifecycle event tests ───────────────────────────────────────────

/// `session.updated` event must parse into `SseEvent::SessionUpdated` with the
/// correct `session_id` extracted from `properties.info.id`.
#[test]
fn test_session_updated_parses_correctly() {
    let block = r#"data: {"type":"session.updated","properties":{"info":{"id":"sess_upd"}}}"#;
    let event = parse_sse_block(block).expect("session.updated should produce an event");
    assert!(
        matches!(event, SseEvent::SessionUpdated { ref session_id } if session_id == "sess_upd"),
        "Expected SessionUpdated with session_id \"sess_upd\", got {:?}",
        event
    );
}

/// `session.created` event must parse into `SseEvent::SessionCreated` with the
/// correct `session_id` extracted from `properties.info.id`.
#[test]
fn test_session_created_parses_correctly() {
    let block = r#"data: {"type":"session.created","properties":{"info":{"id":"sess_new"}}}"#;
    let event = parse_sse_block(block).expect("session.created should produce an event");
    assert!(
        matches!(event, SseEvent::SessionCreated { ref session_id } if session_id == "sess_new"),
        "Expected SessionCreated with session_id \"sess_new\", got {:?}",
        event
    );
}

/// `session.deleted` event must parse into `SseEvent::SessionDeleted` with the
/// correct `session_id` extracted from `properties.info.id`.
#[test]
fn test_session_deleted_parses_correctly() {
    let block = r#"data: {"type":"session.deleted","properties":{"info":{"id":"sess_del"}}}"#;
    let event = parse_sse_block(block).expect("session.deleted should produce an event");
    assert!(
        matches!(event, SseEvent::SessionDeleted { ref session_id } if session_id == "sess_del"),
        "Expected SessionDeleted with session_id \"sess_del\", got {:?}",
        event
    );
}

// ─── Backoff calculation tests ────────────────────────────────────────────────

/// Verify the reconnect delay doubles each step and caps at 30 seconds.
///
/// Sequence starting from 1s: 1 → 2 → 4 → 8 → 16 → 30 → 30 → 30
#[test]
fn test_backoff_doubles_and_caps_at_30s() {
    let mut delay: u64 = 1;
    let mut sequence = Vec::new();
    for _ in 0..8 {
        sequence.push(delay);
        delay = next_reconnect_delay(delay);
    }
    assert_eq!(
        sequence,
        vec![1, 2, 4, 8, 16, 30, 30, 30],
        "Backoff sequence should be 1→2→4→8→16→30→30→30"
    );
}

/// Verify individual delay transitions.
#[test]
fn test_backoff_individual_steps() {
    assert_eq!(next_reconnect_delay(1), 2, "1s → 2s");
    assert_eq!(next_reconnect_delay(2), 4, "2s → 4s");
    assert_eq!(next_reconnect_delay(4), 8, "4s → 8s");
    assert_eq!(next_reconnect_delay(8), 16, "8s → 16s");
    assert_eq!(next_reconnect_delay(16), 30, "16s → 30s (capped)");
    assert_eq!(next_reconnect_delay(30), 30, "30s → 30s (stays capped)");
    assert_eq!(next_reconnect_delay(100), 30, "100s → 30s (capped)");
}

/// Verify the cap is exactly 30 seconds (not 32 or any other power of 2).
#[test]
fn test_backoff_cap_is_exactly_30s() {
    // 15 * 2 = 30 — exactly at cap
    assert_eq!(next_reconnect_delay(15), 30);
    // 16 * 2 = 32 — would exceed cap, should be 30
    assert_eq!(next_reconnect_delay(16), 30);
    // Already at cap
    assert_eq!(next_reconnect_delay(30), 30);
}
