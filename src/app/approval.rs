//! Approval flow integration — SSE event handlers for the approval queue.
//!
//! These functions are called from the main event loop in [`super`] when
//! SSE events arrive from OpenCode.  They update the [`ApprovalQueue`] and
//! transition the recording state machine as needed.
//!
//! Voice-driven reply logic lives in [`super::recording::try_handle_approval`].
//! This module provides the SSE handlers and the shared
//! [`refresh_approval_display`] helper used after a voice reply is dispatched.

use crate::approval::types::{PermissionRequest, QuestionRequest};
use crate::state::RecordingState;

use super::VoiceApp;

/// Called when a `permission.asked` SSE event arrives.
///
/// Adds the request to the approval queue and transitions to
/// [`RecordingState::ApprovalPending`] if not already there.
pub(crate) fn handle_sse_permission_asked(app: &mut VoiceApp, req: PermissionRequest) {
    app.approval_queue.add_permission(req);
    if app.state != RecordingState::ApprovalPending {
        app.state = RecordingState::ApprovalPending;
    }
    app.render_display();
}

/// Called when a `permission.replied` SSE event arrives.
///
/// Removes the matching request from the queue.  If the queue is now empty
/// and the state is still [`RecordingState::ApprovalPending`], transitions
/// back to [`RecordingState::Idle`].
pub(crate) fn handle_sse_permission_replied(
    app: &mut VoiceApp,
    _session_id: &str,
    request_id: &str,
    _reply: &str,
) {
    app.approval_queue.remove(request_id);
    if !app.approval_queue.has_pending() && app.state == RecordingState::ApprovalPending {
        app.state = RecordingState::Idle;
    }
    app.render_display();
}

/// Called when a `question.asked` SSE event arrives.
///
/// Adds the request to the approval queue and transitions to
/// [`RecordingState::ApprovalPending`] if not already there.
pub(crate) fn handle_sse_question_asked(app: &mut VoiceApp, req: QuestionRequest) {
    app.approval_queue.add_question(req);
    if app.state != RecordingState::ApprovalPending {
        app.state = RecordingState::ApprovalPending;
    }
    app.render_display();
}

/// Called when a `question.replied` SSE event arrives.
///
/// Removes the matching request from the queue.  If the queue is now empty
/// and the state is still [`RecordingState::ApprovalPending`], transitions
/// back to [`RecordingState::Idle`].
pub(crate) fn handle_sse_question_replied(
    app: &mut VoiceApp,
    _session_id: &str,
    request_id: &str,
    _answers: Vec<Vec<String>>,
) {
    app.approval_queue.remove(request_id);
    if !app.approval_queue.has_pending() && app.state == RecordingState::ApprovalPending {
        app.state = RecordingState::Idle;
    }
    app.render_display();
}

/// Called when a `question.rejected` SSE event arrives.
///
/// Removes the matching request from the queue.  If the queue is now empty
/// and the state is still [`RecordingState::ApprovalPending`], transitions
/// back to [`RecordingState::Idle`].
pub(crate) fn handle_sse_question_rejected(
    app: &mut VoiceApp,
    _session_id: &str,
    request_id: &str,
) {
    app.approval_queue.remove(request_id);
    if !app.approval_queue.has_pending() && app.state == RecordingState::ApprovalPending {
        app.state = RecordingState::Idle;
    }
    app.render_display();
}

/// Called when a `session.status` SSE event arrives.
///
/// When the session transitions to **busy**, it means the AI has resumed
/// work.  Any pending permissions or questions must have already been
/// answered — possibly by the user interacting directly with the OpenCode
/// TUI rather than through voice.  In that case, the individual
/// `permission.replied` / `question.replied` SSE events *should* also have
/// arrived, but as a safety net (e.g. SSE reconnection gaps) we clear all
/// stale approvals here.
///
/// When the session transitions to **idle** with approvals still in the
/// queue, those approvals are stale (the session finished without us
/// receiving individual reply events) and are also cleared.
pub(crate) fn handle_sse_session_status(app: &mut VoiceApp, _session_id: &str, busy: bool) {
    if busy && app.approval_queue.has_pending() {
        // AI resumed work → all pending approvals were answered externally.
        app.display
            .log("[voice] Session became busy — clearing pending approvals (answered externally).");
        app.approval_queue.clear();
        if app.state == RecordingState::ApprovalPending {
            app.state = RecordingState::Idle;
        }
        app.render_display();
    } else if !busy && app.approval_queue.has_pending() {
        // Session went idle but we still have approvals — they are stale.
        app.display
            .log("[voice] Session idle — clearing stale approvals.");
        app.approval_queue.clear();
        if app.state == RecordingState::ApprovalPending {
            app.state = RecordingState::Idle;
        }
        app.render_display();
    }
}

/// Refreshes the approval display after a voice-driven reply has been sent.
///
/// Applies the following state-machine rules and then re-renders the terminal:
///
/// * If the queue still has pending items **and** the current state is
///   [`RecordingState::Idle`] or [`RecordingState::ApprovalPending`] →
///   transition to (or stay at) [`RecordingState::ApprovalPending`].
/// * If the queue is now empty **and** the current state is
///   [`RecordingState::ApprovalPending`] → transition to
///   [`RecordingState::Idle`].
/// * Otherwise (e.g. Recording, Transcribing, Error) → leave the state
///   unchanged and just re-render.
pub(crate) fn refresh_approval_display(app: &mut VoiceApp) {
    if app.approval_queue.has_pending() {
        if app.state == RecordingState::Idle || app.state == RecordingState::ApprovalPending {
            app.state = RecordingState::ApprovalPending;
        }
    } else if app.state == RecordingState::ApprovalPending {
        app.state = RecordingState::Idle;
    }
    app.render_display();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::VoiceApp;
    use crate::approval::types::{PermissionRequest, QuestionRequest};
    use crate::config::{AppConfig, ModelSize};
    use std::path::PathBuf;

    fn test_config() -> AppConfig {
        AppConfig {
            whisper_model_path: PathBuf::from("/nonexistent/model.bin"),
            opencode_port: 4096,
            toggle_key: ' ',
            model_size: ModelSize::TinyEn,
            server_password: None,
            data_dir: PathBuf::from("/nonexistent/data"),
            audio_device: None,
            use_global_hotkey: false,
            global_hotkey: "right_option".to_string(),
            push_to_talk: true,
            handle_prompts: true,
            debug: false,
        }
    }

    fn make_permission(id: &str) -> PermissionRequest {
        PermissionRequest {
            id: id.to_string(),
            permission: "bash".to_string(),
            metadata: serde_json::Value::Null,
        }
    }

    fn make_question(id: &str) -> QuestionRequest {
        QuestionRequest {
            id: id.to_string(),
            questions: vec![],
        }
    }

    #[test]
    fn test_permission_asked_transitions_to_approval_pending() {
        let mut app = VoiceApp::new(test_config()).unwrap();
        assert_eq!(app.state, RecordingState::Idle);
        handle_sse_permission_asked(&mut app, make_permission("p1"));
        assert_eq!(app.state, RecordingState::ApprovalPending);
        assert!(app.approval_queue.has_pending());
    }

    #[test]
    fn test_permission_replied_removes_from_queue_and_returns_to_idle() {
        let mut app = VoiceApp::new(test_config()).unwrap();
        handle_sse_permission_asked(&mut app, make_permission("p1"));
        assert_eq!(app.state, RecordingState::ApprovalPending);

        handle_sse_permission_replied(&mut app, "sess", "p1", "once");
        assert_eq!(app.state, RecordingState::Idle);
        assert!(!app.approval_queue.has_pending());
    }

    #[test]
    fn test_question_asked_transitions_to_approval_pending() {
        let mut app = VoiceApp::new(test_config()).unwrap();
        handle_sse_question_asked(&mut app, make_question("q1"));
        assert_eq!(app.state, RecordingState::ApprovalPending);
    }

    #[test]
    fn test_question_replied_removes_from_queue() {
        let mut app = VoiceApp::new(test_config()).unwrap();
        handle_sse_question_asked(&mut app, make_question("q1"));
        handle_sse_question_replied(&mut app, "sess", "q1", vec![]);
        assert_eq!(app.state, RecordingState::Idle);
        assert!(!app.approval_queue.has_pending());
    }

    #[test]
    fn test_question_rejected_removes_from_queue() {
        let mut app = VoiceApp::new(test_config()).unwrap();
        handle_sse_question_asked(&mut app, make_question("q1"));
        handle_sse_question_rejected(&mut app, "sess", "q1");
        assert_eq!(app.state, RecordingState::Idle);
        assert!(!app.approval_queue.has_pending());
    }

    // ── handle_sse_session_status ──────────────────────────────────────

    #[test]
    fn test_session_busy_clears_pending_approvals() {
        let mut app = VoiceApp::new(test_config()).unwrap();
        handle_sse_permission_asked(&mut app, make_permission("p1"));
        handle_sse_question_asked(&mut app, make_question("q1"));
        assert_eq!(app.state, RecordingState::ApprovalPending);
        assert_eq!(app.approval_queue.len(), 2);

        // Session becomes busy → AI resumed → approvals were answered externally.
        handle_sse_session_status(&mut app, "sess", true);
        assert_eq!(app.state, RecordingState::Idle);
        assert!(!app.approval_queue.has_pending());
    }

    #[test]
    fn test_session_idle_clears_stale_approvals() {
        let mut app = VoiceApp::new(test_config()).unwrap();
        handle_sse_permission_asked(&mut app, make_permission("p1"));
        assert_eq!(app.state, RecordingState::ApprovalPending);

        // Session went idle with approvals still in queue → stale.
        handle_sse_session_status(&mut app, "sess", false);
        assert_eq!(app.state, RecordingState::Idle);
        assert!(!app.approval_queue.has_pending());
    }

    #[test]
    fn test_session_busy_no_op_without_pending() {
        let mut app = VoiceApp::new(test_config()).unwrap();
        assert_eq!(app.state, RecordingState::Idle);

        // No pending approvals → should be a no-op.
        handle_sse_session_status(&mut app, "sess", true);
        assert_eq!(app.state, RecordingState::Idle);
    }

    #[test]
    fn test_session_busy_does_not_change_recording_state() {
        let mut app = VoiceApp::new(test_config()).unwrap();
        handle_sse_permission_asked(&mut app, make_permission("p1"));
        // Manually set to Recording (user started recording before session went busy).
        app.state = RecordingState::Recording;

        handle_sse_session_status(&mut app, "sess", true);
        // Queue should be cleared but state should stay Recording (not forced to Idle).
        assert!(!app.approval_queue.has_pending());
        assert_eq!(app.state, RecordingState::Recording);
    }

    #[test]
    fn test_session_idle_no_op_without_pending() {
        let mut app = VoiceApp::new(test_config()).unwrap();
        assert_eq!(app.state, RecordingState::Idle);

        // No pending approvals → idle event should be a no-op.
        handle_sse_session_status(&mut app, "sess", false);
        assert_eq!(app.state, RecordingState::Idle);
        assert!(!app.approval_queue.has_pending());
    }

    #[test]
    fn test_session_idle_does_not_change_transcribing_state() {
        let mut app = VoiceApp::new(test_config()).unwrap();
        handle_sse_permission_asked(&mut app, make_permission("p1"));
        // Manually set to Transcribing (user finished recording, transcription in progress).
        app.state = RecordingState::Transcribing;

        handle_sse_session_status(&mut app, "sess", false);
        // Queue should be cleared but state should stay Transcribing (not forced to Idle).
        assert!(!app.approval_queue.has_pending());
        assert_eq!(app.state, RecordingState::Transcribing);
    }

    #[test]
    fn test_multiple_approvals_stay_pending_until_all_cleared() {
        let mut app = VoiceApp::new(test_config()).unwrap();
        handle_sse_permission_asked(&mut app, make_permission("p1"));
        handle_sse_question_asked(&mut app, make_question("q1"));
        assert_eq!(app.approval_queue.len(), 2);

        handle_sse_permission_replied(&mut app, "sess", "p1", "once");
        // Still one item left — should remain ApprovalPending.
        assert_eq!(app.state, RecordingState::ApprovalPending);

        handle_sse_question_rejected(&mut app, "sess", "q1");
        // Queue now empty — should return to Idle.
        assert_eq!(app.state, RecordingState::Idle);
    }
}
