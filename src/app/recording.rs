//! Recording session management — async input event handlers for the recording state machine.
//!
//! These functions are called from the main event loop in [`super`] when
//! keyboard or hotkey input events arrive.  They implement the full recording
//! pipeline: cpal capture → Whisper transcription → OpenCode injection.
//!
//! # Push-to-talk flow
//!
//! 1. [`handle_push_to_talk_start`] — opens a [`CpalRecorder`], starts the
//!    audio stream, spawns an energy-forwarding task, transitions to
//!    [`RecordingState::Recording`].
//! 2. [`handle_push_to_talk_stop`] — stops the recorder, checks minimum
//!    duration, writes a [`TempWav`], transcribes via Whisper, injects into
//!    OpenCode, transitions back to Idle (or ApprovalPending).
//!
use crate::audio::capture::CpalRecorder;
use crate::audio::wav::TempWav;
use crate::state::{AppEvent, RecordingState};

use super::VoiceApp;

/// A `Send`-able wrapper around a raw pointer to [`WhisperEngine`].
///
/// # Safety
///
/// The caller must guarantee that:
/// 1. The pointed-to `WhisperEngine` outlives all tasks that hold this wrapper.
/// 2. The engine is never mutated while tasks are running.
/// 3. No two tasks call `transcribe` concurrently on the same engine
///    (whisper-rs is not thread-safe for concurrent inference).
///
/// In practice the engine is owned by `VoiceApp` which lives for the entire
/// duration of the program, and we only ever run one transcription at a time.
struct SendWhisperPtr(*const crate::transcribe::engine::WhisperEngine);

// SAFETY: see doc comment above.
unsafe impl Send for SendWhisperPtr {}

impl SendWhisperPtr {
    /// Returns a shared reference to the pointed-to engine.
    ///
    /// # Safety
    ///
    /// The caller must ensure the pointer is valid and the engine is not
    /// concurrently mutated.
    unsafe fn as_ref(&self) -> &crate::transcribe::engine::WhisperEngine {
        &*self.0
    }
}

// Minimum recording duration in seconds.  Recordings shorter than this are
// silently discarded (e.g. accidental key taps).
const MIN_RECORDING_SECS: f64 = 0.5;

// Minimum number of i16 samples required to attempt transcription.
// At 16 kHz mono: 0.5 s × 16 000 = 8 000 samples.
const MIN_SAMPLES: usize = 8_000;

// ─── Public entry points ────────────────────────────────────────────────────

/// Handles a toggle event in standard (non-push-to-talk) mode.
///
/// Starts recording from [`RecordingState::Idle`] or
/// [`RecordingState::ApprovalPending`].  Stops recording from
/// [`RecordingState::Recording`].  Other states are ignored.
pub(crate) async fn handle_toggle(app: &mut VoiceApp) {
    match app.state {
        RecordingState::Idle | RecordingState::ApprovalPending => {
            app.state = RecordingState::Recording;
            app.current_level = None;
            app.render_display();
        }
        RecordingState::Recording => {
            app.state = RecordingState::Transcribing;
            app.current_level = None;
            app.render_display();
        }
        _ => {
            // Ignore toggle in other states.
        }
    }
}


/// Starts push-to-talk recording (key pressed down).
///
/// Opens a [`CpalRecorder`] for the configured audio device, starts the
/// stream, spawns a task that forwards RMS energy values as
/// [`AppEvent::AudioChunk`] events, and transitions to
/// [`RecordingState::Recording`].
///
/// Recording is allowed from both [`RecordingState::Idle`] and
/// [`RecordingState::ApprovalPending`].  In the latter case the user may
/// be speaking to answer a pending approval, or to inject a new prompt —
/// the transcription pipeline handles both.
///
/// If the recorder cannot be opened (e.g. no microphone), the error is
/// reported via [`VoiceApp::handle_error`] and the state remains unchanged.
pub(crate) async fn handle_push_to_talk_start(app: &mut VoiceApp) {
    if app.state != RecordingState::Idle && app.state != RecordingState::ApprovalPending {
        return;
    }

    let device = app.audio_config.device.as_deref();

    // Create and start the recorder.
    let mut recorder = match CpalRecorder::new(device) {
        Ok(r) => r,
        Err(e) => {
            app.handle_error(&format!("Failed to open audio device: {}", e));
            return;
        }
    };

    let energy_rx = match recorder.start() {
        Ok(rx) => rx,
        Err(e) => {
            app.handle_error(&format!("Failed to start recording: {}", e));
            return;
        }
    };

    // Spawn a task that reads RMS energy from the recorder and forwards it to
    // the event loop as AudioChunk events so the level meter stays live.
    let event_tx = app.event_tx.clone();
    let mut energy_rx = energy_rx;
    tokio::spawn(async move {
        while let Some(rms_energy) = energy_rx.recv().await {
            if event_tx
                .send(AppEvent::AudioChunk { rms_energy })
                .is_err()
            {
                break; // Event loop has shut down.
            }
        }
    });

    // Store the recorder so handle_push_to_talk_stop can retrieve it.
    app.recorder = Some(recorder);

    {
        let name = app.recorder.as_ref()
            .and_then(|r| r.device_name())
            .unwrap_or("unknown");
        app.debug_log(format_args!("recording started  device: {}", name));
    }

    app.state = RecordingState::Recording;
    app.current_level = None;
    app.render_display();
}

/// Stops push-to-talk recording (key released).
///
/// Retrieves the active [`CpalRecorder`], stops it, checks the minimum
/// recording duration, writes a [`TempWav`], transcribes via Whisper, and
/// injects the result into OpenCode.  Transitions back to Idle (or
/// [`RecordingState::ApprovalPending`] if there are pending approvals).
///
/// Short recordings (< 0.5 s) are silently discarded.
pub(crate) async fn handle_push_to_talk_stop(app: &mut VoiceApp) {
    if app.state != RecordingState::Recording {
        return;
    }

    // Take the recorder out of the app struct.
    let mut recorder = match app.recorder.take() {
        Some(r) => r,
        None => {
            // No recorder — just return to idle.
            return_to_idle_or_approval(app);
            return;
        }
    };

    // Check duration before stopping (stop() clears the start_time).
    let duration = recorder.duration();

    // Stop the stream and collect samples.
    let samples = match recorder.stop() {
        Ok(s) => s,
        Err(e) => {
            app.handle_error(&format!("Failed to stop recording: {}", e));
            return;
        }
    };

    app.debug_log(format_args!("recording stopped  duration: {:.2}s  samples: {}", duration, samples.len()));

    // Discard very short recordings.
    if duration < MIN_RECORDING_SECS || samples.len() < MIN_SAMPLES {
        app.display.log(&format!(
            "[voice] Recording too short ({:.2}s, {} samples) — discarded.",
            duration,
            samples.len()
        ));
        return_to_idle_or_approval(app);
        return;
    }

    // Transition to Transcribing while we process.
    app.state = RecordingState::Transcribing;
    app.current_level = None;
    app.render_display();

    // Write samples to a temporary WAV file.
    // TempWav is RAII: if we return early (error path) before calling
    // into_path(), the file is automatically deleted on drop.
    let wav = TempWav::new();
    if let Err(e) = wav.write(&samples, &app.audio_config) {
        app.handle_error(&format!("Failed to write WAV file: {}", e));
        return;
    }

    // Consume TempWav without deleting the file; we own cleanup from here.
    let wav_path = wav.into_path();

    // Run Whisper transcription on a blocking thread (CPU-bound).
    let transcript = match &app.whisper {
        None => {
            // No model loaded — clean up and return.
            let _ = std::fs::remove_file(&wav_path);
            app.handle_error("Whisper model not loaded. Run 'opencode-voice setup'.");
            return;
        }
        Some(_) => {
            // Clone the path for the blocking closure.
            let path_for_task = wav_path.clone();

            // SAFETY: We need to move the WhisperEngine reference into the
            // blocking task.  We use a raw pointer wrapped in SendWhisperPtr
            // to work around the borrow checker — this is safe because:
            //   1. We await the task before returning, so the engine outlives it.
            //   2. The task does not outlive this function frame.
            //   3. WhisperEngine is not mutated.
            let engine_ptr = SendWhisperPtr(
                app.whisper.as_ref().unwrap() as *const crate::transcribe::engine::WhisperEngine,
            );

            let result = tokio::task::spawn_blocking(move || {
                // SAFETY: see SendWhisperPtr safety doc.
                let engine = unsafe { engine_ptr.as_ref() };
                engine.transcribe(&path_for_task)
            })
            .await;

            // Clean up the WAV file regardless of transcription outcome.
            let _ = std::fs::remove_file(&wav_path);

            match result {
                Ok(Ok(r)) => r,
                Ok(Err(e)) => {
                    app.handle_error(&format!("Transcription failed: {}", e));
                    return;
                }
                Err(e) => {
                    app.handle_error(&format!("Transcription task panicked: {}", e));
                    return;
                }
            }
        }
    };

    let text = transcript.text.trim().to_string();

    if app.config.debug {
        if text.is_empty() {
            app.debug_log(format_args!("transcript: (empty)"));
        } else {
            app.debug_log(format_args!("transcript: {}", text));
        }
        // In debug mode, skip OpenCode injection entirely.
        app.last_transcript = if text.is_empty() { None } else { Some(text) };
        return_to_idle_or_approval(app);
        return;
    }

    if text.is_empty() {
        // Nothing transcribed — return to idle without injecting.
        return_to_idle_or_approval(app);
        return;
    }

    // Store the transcript for the idle display.
    app.last_transcript = Some(text.clone());

    // Check if there is a pending approval that this text might answer.
    if app.approval_queue.has_pending() {
        let handled = try_handle_approval(app, &text).await;
        if handled {
            return_to_idle_or_approval(app);
            return;
        }
    }

    // Inject the transcribed text into OpenCode.
    inject_text(app, &text).await;
}

// ─── Helpers ────────────────────────────────────────────────────────────────

/// Sends `text` to the active OpenCode session and transitions to Injecting → Idle.
///
/// If no active session exists, creates one first. On error, calls `handle_error`.
async fn inject_text(app: &mut VoiceApp, text: &str) {
    app.state = RecordingState::Injecting;
    app.render_display();

    // Ensure we have an active session.
    if app.active_session.is_none() {
        match app.bridge.create_session().await {
            Ok(session) => {
                eprintln!("[voice] Created session: {}", session.id);
                app.active_session = Some(session.id);
            }
            Err(e) => {
                app.handle_error(&format!("Failed to create session: {}", e));
                return;
            }
        }
    }

    let session_id = app.active_session.as_ref().unwrap().clone();
    if let Err(e) = app.bridge.send_message(&session_id, text).await {
        app.handle_error(&format!("Failed to send message: {}", e));
        return;
    }

    return_to_idle_or_approval(app);
}

/// Transitions to [`RecordingState::ApprovalPending`] if there are pending
/// approvals, otherwise to [`RecordingState::Idle`].  Updates the display.
pub(crate) fn return_to_idle_or_approval(app: &mut VoiceApp) {
    if app.approval_queue.has_pending() {
        app.state = RecordingState::ApprovalPending;
    } else {
        app.state = RecordingState::Idle;
    }
    app.current_level = None;
    app.render_display();
}

/// Attempts to handle `text` as a voice reply to the front-most pending approval.
///
/// # Behaviour
///
/// 1. Peeks the approval queue.  If empty, returns `false` immediately so the
///    caller can fall through to normal prompt injection.
/// 2. **Permission** — calls [`match_permission_command`].  On a match, sends
///    the reply via [`OpenCodeBridge::reply_permission`], removes the item from
///    the queue, calls [`refresh_approval_display`], and returns `true`.
///    On [`MatchResult::NoMatch`] returns `false`.
/// 3. **Question** — calls [`match_question_answer`].
///    * [`MatchResult::QuestionAnswer`] → [`OpenCodeBridge::reply_question`] →
///      remove → refresh → `true`.
///    * [`MatchResult::QuestionReject`] → [`OpenCodeBridge::reject_question`] →
///      remove → refresh → `true`.
///    * [`MatchResult::NoMatch`] → `false`.
///
/// Bridge call failures are reported via [`VoiceApp::handle_error`] (non-fatal)
/// and the function still returns `true` so the text is not re-injected as a
/// normal prompt.
pub(crate) async fn try_handle_approval(app: &mut VoiceApp, text: &str) -> bool {
    use crate::approval::matcher::{match_permission_command, match_question_answer, MatchResult};
    use crate::approval::types::PendingApproval;

    // Peek at the front of the queue.  Clone what we need so we can release
    // the borrow on `app` before making async bridge calls.
    let pending = match app.approval_queue.peek() {
        Some(p) => p.clone(),
        None => return false,
    };

    match &pending {
        PendingApproval::Permission(_req) => {
            let result = match_permission_command(text);
            match result {
                MatchResult::PermissionReply { reply, message } => {
                    let id = pending.id().to_string();
                    let msg_ref = message.as_deref();
                    if let Err(e) = app.bridge.reply_permission(&id, reply, msg_ref).await {
                        app.handle_error(&format!("Failed to reply to permission: {}", e));
                    }
                    app.approval_queue.remove(&id);
                    super::approval::refresh_approval_display(app);
                    true
                }
                MatchResult::NoMatch => false,
                // match_permission_command never returns QuestionAnswer / QuestionReject,
                // but the compiler requires exhaustive matching.
                _ => false,
            }
        }

        PendingApproval::Question(req) => {
            // Clone the request so we can pass it to the matcher without
            // holding a borrow on `app`.
            let req_clone = req.clone();
            let result = match_question_answer(text, &req_clone);
            match result {
                MatchResult::QuestionAnswer { answers } => {
                    let id = pending.id().to_string();
                    if let Err(e) = app.bridge.reply_question(&id, answers).await {
                        app.handle_error(&format!("Failed to reply to question: {}", e));
                    }
                    app.approval_queue.remove(&id);
                    super::approval::refresh_approval_display(app);
                    true
                }
                MatchResult::QuestionReject => {
                    let id = pending.id().to_string();
                    if let Err(e) = app.bridge.reject_question(&id).await {
                        app.handle_error(&format!("Failed to reject question: {}", e));
                    }
                    app.approval_queue.remove(&id);
                    super::approval::refresh_approval_display(app);
                    true
                }
                MatchResult::NoMatch => false,
                // match_question_answer never returns PermissionReply.
                _ => false,
            }
        }
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::VoiceApp;
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
            push_to_talk: false,
            handle_prompts: false,
            debug: false,
        }
    }

    // ── handle_toggle ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_handle_toggle_idle_to_recording() {
        let mut app = VoiceApp::new(test_config()).unwrap();
        assert_eq!(app.state, RecordingState::Idle);
        handle_toggle(&mut app).await;
        assert_eq!(app.state, RecordingState::Recording);
    }

    #[tokio::test]
    async fn test_handle_toggle_recording_to_transcribing() {
        let mut app = VoiceApp::new(test_config()).unwrap();
        app.state = RecordingState::Recording;
        handle_toggle(&mut app).await;
        assert_eq!(app.state, RecordingState::Transcribing);
    }

    #[tokio::test]
    async fn test_handle_toggle_approval_pending_to_recording() {
        let mut app = VoiceApp::new(test_config()).unwrap();
        app.state = RecordingState::ApprovalPending;
        handle_toggle(&mut app).await;
        assert_eq!(app.state, RecordingState::Recording);
    }

    #[tokio::test]
    async fn test_handle_toggle_ignores_transcribing_state() {
        let mut app = VoiceApp::new(test_config()).unwrap();
        app.state = RecordingState::Transcribing;
        handle_toggle(&mut app).await;
        assert_eq!(app.state, RecordingState::Transcribing);
    }

    // ── handle_push_to_talk_start / stop ─────────────────────────────────────

    #[tokio::test]
    async fn test_handle_push_to_talk_start_ignores_transcribing() {
        let mut app = VoiceApp::new(test_config()).unwrap();
        app.state = RecordingState::Transcribing;
        handle_push_to_talk_start(&mut app).await;
        // Should remain Transcribing — PTT start is only allowed from Idle or ApprovalPending.
        assert_eq!(app.state, RecordingState::Transcribing);
    }

    #[tokio::test]
    async fn test_handle_push_to_talk_start_ignores_recording() {
        let mut app = VoiceApp::new(test_config()).unwrap();
        app.state = RecordingState::Recording;
        handle_push_to_talk_start(&mut app).await;
        // Should remain Recording — PTT start is only allowed from Idle or ApprovalPending.
        assert_eq!(app.state, RecordingState::Recording);
    }

    #[tokio::test]
    async fn test_handle_push_to_talk_stop_ignores_idle() {
        let mut app = VoiceApp::new(test_config()).unwrap();
        // Calling stop when not recording should be a no-op.
        handle_push_to_talk_stop(&mut app).await;
        assert_eq!(app.state, RecordingState::Idle);
    }

    #[tokio::test]
    async fn test_handle_push_to_talk_stop_no_recorder_returns_to_idle() {
        let mut app = VoiceApp::new(test_config()).unwrap();
        // Manually set Recording state without a recorder.
        app.state = RecordingState::Recording;
        handle_push_to_talk_stop(&mut app).await;
        // Should return to Idle (no recorder → return_to_idle_or_approval).
        assert_eq!(app.state, RecordingState::Idle);
    }

    // ── return_to_idle_or_approval ───────────────────────────────────────────

    #[test]
    fn test_return_to_idle_when_no_pending() {
        let mut app = VoiceApp::new(test_config()).unwrap();
        app.state = RecordingState::Injecting;
        return_to_idle_or_approval(&mut app);
        assert_eq!(app.state, RecordingState::Idle);
    }

    #[test]
    fn test_return_to_approval_pending_when_queue_has_items() {
        use crate::approval::types::PermissionRequest;

        let mut app = VoiceApp::new(test_config()).unwrap();
        app.state = RecordingState::Injecting;

        // Add a pending approval.
        app.approval_queue.add_permission(PermissionRequest {
            id: "p1".to_string(),
            permission: "bash".to_string(),
            metadata: serde_json::Value::Null,
        });

        return_to_idle_or_approval(&mut app);
        assert_eq!(app.state, RecordingState::ApprovalPending);
    }

    // ── inject_text ─────────────────────────────────────────────────────────────

    /// With an active session set, inject_text sends the message and transitions
    /// to Idle on success.
    #[tokio::test]
    async fn test_inject_text_sends_message_to_active_session() {
        use wiremock::{Mock, MockServer, ResponseTemplate};
        use wiremock::matchers::{method, path};

        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/session/sess_1/prompt_async"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&mock_server)
            .await;

        let mut config = test_config();
        config.opencode_port = mock_server.address().port();
        let mut app = VoiceApp::new(config).unwrap();
        app.active_session = Some("sess_1".to_string());

        inject_text(&mut app, "hello world").await;

        assert_eq!(app.state, RecordingState::Idle);
        assert_eq!(mock_server.received_requests().await.unwrap().len(), 1);
    }

    /// With no active session, inject_text creates one via the bridge and then
    /// sends the message, ending in Idle with the new session stored.
    #[tokio::test]
    async fn test_inject_text_creates_session_when_none() {
        use wiremock::{Mock, MockServer, ResponseTemplate};
        use wiremock::matchers::{method, path};

        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/session"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"id": "new_sess", "title": "New"})),
            )
            .mount(&mock_server)
            .await;

        Mock::given(method("POST"))
            .and(path("/session/new_sess/prompt_async"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&mock_server)
            .await;

        let mut config = test_config();
        config.opencode_port = mock_server.address().port();
        let mut app = VoiceApp::new(config).unwrap();
        assert!(app.active_session.is_none());

        inject_text(&mut app, "test text").await;

        assert_eq!(app.state, RecordingState::Idle);
        assert_eq!(app.active_session, Some("new_sess".to_string()));
    }

    /// When send_message returns a server error, inject_text transitions to Error.
    #[tokio::test]
    async fn test_inject_text_handles_send_message_failure() {
        use wiremock::{Mock, MockServer, ResponseTemplate};
        use wiremock::matchers::{method, path};

        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/session/sess_1/prompt_async"))
            .respond_with(
                ResponseTemplate::new(500)
                    .set_body_json(serde_json::json!({"error": "server error"})),
            )
            .mount(&mock_server)
            .await;

        let mut config = test_config();
        config.opencode_port = mock_server.address().port();
        let mut app = VoiceApp::new(config).unwrap();
        app.active_session = Some("sess_1".to_string());

        inject_text(&mut app, "will fail").await;

        assert_eq!(app.state, RecordingState::Error);
    }

    // ── try_handle_approval ──────────────────────────────────────────────────

    /// Returns false when the approval queue is empty (nothing to handle).
    #[tokio::test]
    async fn test_try_handle_approval_empty_queue_returns_false() {
        let mut app = VoiceApp::new(test_config()).unwrap();
        // Queue is empty — any text should return false.
        let result = try_handle_approval(&mut app, "yes").await;
        assert!(!result, "empty queue should return false");
    }

    /// Returns false when the text does not match any permission pattern.
    #[tokio::test]
    async fn test_try_handle_approval_permission_no_match_returns_false() {
        use crate::approval::types::PermissionRequest;

        let mut app = VoiceApp::new(test_config()).unwrap();
        app.approval_queue.add_permission(PermissionRequest {
            id: "p1".to_string(),
            permission: "bash".to_string(),
            metadata: serde_json::Value::Null,
        });

        // "hello world" does not match any permission command.
        let result = try_handle_approval(&mut app, "hello world").await;
        assert!(!result, "unrecognised text should return false");
        // Item must still be in the queue.
        assert!(app.approval_queue.has_pending());
    }

    /// Returns false when the text does not match any question option.
    #[tokio::test]
    async fn test_try_handle_approval_question_no_match_returns_false() {
        use crate::approval::types::{QuestionInfo, QuestionOption, QuestionRequest};

        let mut app = VoiceApp::new(test_config()).unwrap();
        app.approval_queue.add_question(QuestionRequest {
            id: "q1".to_string(),
            questions: vec![QuestionInfo {
                question: "Pick one".to_string(),
                options: vec![
                    QuestionOption {
                        label: "Alpha".to_string(),
                    },
                    QuestionOption {
                        label: "Beta".to_string(),
                    },
                ],
                custom: false, // no custom answers allowed
            }],
        });

        // "gamma" is not an option and custom is disabled.
        let result = try_handle_approval(&mut app, "gamma").await;
        assert!(!result, "unrecognised question answer should return false");
        assert!(app.approval_queue.has_pending());
    }

    /// A matching permission command removes the item from the queue and
    /// returns true.  Uses a wiremock mock server to verify the bridge call
    /// to `POST /permission/p1/reply` is made with the correct body.
    #[tokio::test]
    async fn test_try_handle_approval_permission_match_removes_item_and_returns_true() {
        use wiremock::{Mock, MockServer, ResponseTemplate};
        use wiremock::matchers::{method, path};
        use crate::approval::types::PermissionRequest;

        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/permission/p1/reply"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .mount(&mock_server)
            .await;

        let mut config = test_config();
        config.opencode_port = mock_server.address().port();
        let mut app = VoiceApp::new(config).unwrap();
        app.approval_queue.add_permission(PermissionRequest {
            id: "p1".to_string(),
            permission: "bash".to_string(),
            metadata: serde_json::Value::Null,
        });
        app.state = RecordingState::ApprovalPending;

        // "yes" matches the Once permission pattern and triggers POST /permission/p1/reply.
        let result = try_handle_approval(&mut app, "yes").await;
        assert!(result, "matched permission should return true");
        assert!(
            !app.approval_queue.has_pending(),
            "item should be removed from queue after match"
        );

        let requests = mock_server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1, "expected exactly 1 bridge request");
        assert_eq!(requests[0].url.path(), "/permission/p1/reply");
        let body = std::str::from_utf8(&requests[0].body).unwrap();
        assert!(
            body.contains("\"reply\"") && body.contains("\"once\""),
            "request body should contain reply:once, got: {body}"
        );
    }

    /// A matching question answer removes the item from the queue and returns
    /// true.  Uses a wiremock mock server to verify the bridge call to
    /// `POST /question/q1/reply` is made.
    #[tokio::test]
    async fn test_try_handle_approval_question_match_removes_item_and_returns_true() {
        use wiremock::{Mock, MockServer, ResponseTemplate};
        use wiremock::matchers::{method, path};
        use crate::approval::types::{QuestionInfo, QuestionOption, QuestionRequest};

        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/question/q1/reply"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .mount(&mock_server)
            .await;

        let mut config = test_config();
        config.opencode_port = mock_server.address().port();
        let mut app = VoiceApp::new(config).unwrap();
        app.approval_queue.add_question(QuestionRequest {
            id: "q1".to_string(),
            questions: vec![QuestionInfo {
                question: "Pick one".to_string(),
                options: vec![
                    QuestionOption {
                        label: "Alpha".to_string(),
                    },
                    QuestionOption {
                        label: "Beta".to_string(),
                    },
                ],
                custom: false,
            }],
        });
        app.state = RecordingState::ApprovalPending;

        // "alpha" matches the first option exactly and triggers POST /question/q1/reply.
        let result = try_handle_approval(&mut app, "alpha").await;
        assert!(result, "matched question answer should return true");
        assert!(
            !app.approval_queue.has_pending(),
            "item should be removed from queue after match"
        );

        let requests = mock_server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1, "expected exactly 1 bridge request");
    }

    /// A question rejection phrase removes the item from the queue and returns
    /// true.  Uses a wiremock mock server to verify the bridge call to
    /// `POST /question/q2/reject` is made.
    #[tokio::test]
    async fn test_try_handle_approval_question_reject_removes_item_and_returns_true() {
        use wiremock::{Mock, MockServer, ResponseTemplate};
        use wiremock::matchers::{method, path};
        use crate::approval::types::{QuestionInfo, QuestionOption, QuestionRequest};

        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/question/q2/reject"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .mount(&mock_server)
            .await;

        let mut config = test_config();
        config.opencode_port = mock_server.address().port();
        let mut app = VoiceApp::new(config).unwrap();
        app.approval_queue.add_question(QuestionRequest {
            id: "q2".to_string(),
            questions: vec![QuestionInfo {
                question: "Pick one".to_string(),
                options: vec![QuestionOption {
                    label: "Yes".to_string(),
                }],
                custom: false,
            }],
        });
        app.state = RecordingState::ApprovalPending;

        // "skip" is a question rejection phrase and triggers POST /question/q2/reject.
        let result = try_handle_approval(&mut app, "skip").await;
        assert!(result, "question rejection should return true");
        assert!(
            !app.approval_queue.has_pending(),
            "item should be removed from queue after rejection"
        );

        let requests = mock_server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1, "expected exactly 1 bridge request");
    }

}
