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
            auto_submit: true,
            server_password: None,
            data_dir: PathBuf::from("/nonexistent/data"),
            audio_device: None,
            use_global_hotkey: false,
            global_hotkey: "right_option".to_string(),
            push_to_talk: true,
            approval_mode: true,
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
