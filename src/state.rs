//! Core state and event types for the voice application.

/// The recording state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordingState {
    Idle,
    Recording,
    Transcribing,
    Injecting,
    ApprovalPending,
    Error,
}

/// Input events from keyboard or global hotkey.
#[derive(Debug, Clone)]
pub enum InputEvent {
    Toggle,
    KeyDown,
    KeyUp,
    Quit,
}

/// Application-wide events flowing through the main event loop channel.
#[derive(Debug)]
pub enum AppEvent {
    Input(InputEvent),
    SseConnected,
    SseDisconnected(Option<String>),
    PermissionAsked(crate::approval::types::PermissionRequest),
    PermissionReplied {
        session_id: String,
        request_id: String,
        reply: String,
    },
    QuestionAsked(crate::approval::types::QuestionRequest),
    QuestionReplied {
        session_id: String,
        request_id: String,
        answers: Vec<Vec<String>>,
    },
    QuestionRejected {
        session_id: String,
        request_id: String,
    },
    AudioChunk {
        rms_energy: f32,
    },
    /// Sent after a 3-second delay to transition back to Idle from Error state.
    RecoverFromError,
    Shutdown,
}
