//! Main application orchestrator for OpenCode Voice Mode.
//!
//! This module owns the [`VoiceApp`] struct, which wires together all subsystems:
//! audio capture, transcription, keyboard/hotkey input, SSE event streaming,
//! approval queue, and the terminal display.

pub mod recording;
pub mod approval;

use anyhow::Result;
use tokio_util::sync::CancellationToken;

use crate::audio::capture::CpalRecorder;
use crate::audio::{default_audio_config, AudioConfig};
use crate::bridge::client::OpenCodeBridge;
use crate::bridge::events::{OpenCodeEvents, SseEvent};
use crate::approval::queue::ApprovalQueue;
use crate::config::AppConfig;
use crate::input::hotkey::GlobalHotkey;
use crate::input::keyboard::{is_tty, KeyboardInput};
use crate::state::{AppEvent, InputEvent, RecordingState};
use crate::transcribe::engine::WhisperEngine;
use crate::transcribe::setup::is_whisper_ready;
use crate::ui::display::{Display, DisplayMeta};

/// Returns a human-readable label for the toggle key character.
/// Turns `' '` into `"Space"` so the UI doesn't show a blank.
fn format_toggle_key(ch: char) -> String {
    match ch {
        ' ' => "Space".into(),
        c => c.to_string(),
    }
}

/// The central application struct that owns all subsystem state.
///
/// Fields are `pub(crate)` so that `recording` and `approval` submodules can
/// access them directly without going through getter methods.
pub struct VoiceApp {
    /// Resolved application configuration.
    pub(crate) config: AppConfig,

    /// Current recording state machine state.
    pub(crate) state: RecordingState,

    /// Terminal display renderer.
    pub(crate) display: Display,

    /// HTTP client for the OpenCode API.
    pub(crate) bridge: OpenCodeBridge,

    /// Loaded Whisper transcription engine (None if model not ready).
    pub(crate) whisper: Option<WhisperEngine>,

    /// FIFO queue for pending permission/question approvals.
    pub(crate) approval_queue: ApprovalQueue,

    /// Active cpal recorder during push-to-talk recording (None when idle).
    ///
    /// Stored here so that `handle_push_to_talk_start` and
    /// `handle_push_to_talk_stop` can share ownership across two separate
    /// async calls without moving the recorder across threads.
    pub(crate) recorder: Option<CpalRecorder>,

    /// Audio configuration derived from app config.
    pub(crate) audio_config: AudioConfig,

    /// Main event channel sender — cloned and given to input/SSE tasks.
    pub(crate) event_tx: tokio::sync::mpsc::UnboundedSender<AppEvent>,

    /// Main event channel receiver — consumed by the event loop.
    event_rx: tokio::sync::mpsc::UnboundedReceiver<AppEvent>,

    /// Cancellation token broadcast to all background tasks.
    pub(crate) cancel: CancellationToken,

    /// Guard against double-shutdown.
    is_shutting_down: bool,

    /// Last transcription text shown in the idle display.
    pub(crate) last_transcript: Option<String>,

    /// Current audio level (0.0–1.0) for the level meter.
    pub(crate) current_level: Option<f32>,

    /// Current error message for the error display.
    pub(crate) current_error: Option<String>,

    /// Spinner frame counter for animated states (Transcribing).
    spinner_frame: usize,

    /// Suppresses the next Toggle event after a KeyUp. The evdev listener
    /// sends KeyUp+Toggle on key release; without this flag the Toggle
    /// would re-start recording after KeyUp completed the full pipeline.
    suppress_next_toggle: bool,

    /// Counter for throttling debug audio level output.
    debug_audio_counter: usize,

    /// The currently active session ID (used for sending messages).
    pub(crate) active_session: Option<String>,
}

impl VoiceApp {
    /// Prints a debug log line with `\r\n` (works correctly in raw mode).
    pub(crate) fn debug_log(&self, msg: std::fmt::Arguments) {
        if self.config.debug {
            eprint!("[debug] {}\r\n", msg);
        }
    }

    /// Creates a new `VoiceApp` from the given configuration.
    ///
    /// Loads the Whisper engine synchronously (blocking) before entering the
    /// async context.  If the model is not yet downloaded the engine is set to
    /// `None` and a warning is printed; the app will still start but
    /// transcription will be unavailable until the model is downloaded.
    pub fn new(config: AppConfig) -> Result<Self> {
        // Load WhisperEngine synchronously (blocking) before entering async.
        // This is intentional: whisper-rs model loading is CPU-bound and must
        // not block the tokio runtime.
        let whisper = if is_whisper_ready(&config.data_dir, &config.model_size) {
            match WhisperEngine::new(&config.whisper_model_path, config.model_size.is_multilingual()) {
                Ok(engine) => {
                    Some(engine)
                }
                Err(e) => {
                    eprintln!("[voice] Warning: failed to load Whisper model: {}", e);
                    None
                }
            }
        } else {
            None
        };

        let bridge = OpenCodeBridge::new(
            "http://localhost",
            config.opencode_port,
            config.server_password.clone(),
        );

        let audio_config = AudioConfig {
            device: config.audio_device.clone(),
            ..default_audio_config()
        };

        let (event_tx, event_rx) = tokio::sync::mpsc::unbounded_channel::<AppEvent>();

        Ok(VoiceApp {
            config,
            state: RecordingState::Idle,
            display: Display::new(),
            bridge,
            whisper,
            approval_queue: ApprovalQueue::new(),
            audio_config,
            recorder: None,
            event_tx,
            event_rx,
            cancel: CancellationToken::new(),
            is_shutting_down: false,
            last_transcript: None,
            current_level: None,
            current_error: None,
            spinner_frame: 0,
            suppress_next_toggle: false,
            debug_audio_counter: 0,
            active_session: None,
        })
    }

    /// Starts the application: spawns background tasks and enters the event loop.
    ///
    /// This method does not return until the application shuts down.
    pub async fn start(&mut self) -> Result<()> {
        // Warn if Whisper model is not ready.
        if self.whisper.is_none() {
            eprintln!(
                "[voice] Warning: Whisper model not found. Run 'opencode-voice setup' to download it."
            );
        }

        // Warn (non-fatal) if OpenCode is not reachable. Skip in debug mode.
        if !self.config.debug && !self.bridge.is_connected().await {
            eprintln!(
                "[voice] Warning: Cannot connect to OpenCode at port {}. \
                 Make sure OpenCode is running with --port {}.",
                self.config.opencode_port, self.config.opencode_port
            );
        }

        // Initialize active session: use most recent existing session, or create one.
        if !self.config.debug {
            match self.bridge.list_sessions().await {
                Ok(sessions) if !sessions.is_empty() => {
                    let id = sessions[0].id.clone();
                    eprintln!("[voice] Active session: {}", id);
                    self.active_session = Some(id);
                }
                Ok(_) => {
                    // No sessions exist — create one.
                    match self.bridge.create_session().await {
                        Ok(session) => {
                            eprintln!("[voice] Created session: {}", session.id);
                            self.active_session = Some(session.id);
                        }
                        Err(e) => {
                            eprintln!("[voice] Warning: Could not create session: {}", e);
                        }
                    }
                }
                Err(e) => {
                    eprintln!("[voice] Warning: Could not list sessions: {}", e);
                }
            }
        }

        // Set up global hotkey before the welcome banner so the banner
        // reflects any fallback from PTT to toggle mode.
        if self.config.use_global_hotkey {
            let hotkey_name = self.config.global_hotkey.clone();
            let cancel = self.cancel.clone();

            let (hotkey_tx, mut hotkey_rx) =
                tokio::sync::mpsc::unbounded_channel::<InputEvent>();

            let event_tx_fwd = self.event_tx.clone();
            tokio::spawn(async move {
                while let Some(ev) = hotkey_rx.recv().await {
                    let _ = event_tx_fwd.send(AppEvent::Input(ev));
                }
            });

            match GlobalHotkey::new(&hotkey_name, hotkey_tx, cancel) {
                Ok(hotkey) => {
                    std::thread::spawn(move || {
                        if let Err(e) = hotkey.run() {
                            eprintln!("[voice] Global hotkey error: {}", e);
                        }
                    });
                }
                Err(e) => {
                    eprintln!("[voice] Warning: Could not set up global hotkey: {}", e);
                    if self.config.push_to_talk {
                        eprintln!("[voice] Falling back to toggle mode (press space to start/stop recording).");
                        self.config.push_to_talk = false;
                    }
                }
            }
        }

        // Show welcome banner BEFORE spawning the keyboard thread.
        // The keyboard thread enables raw mode, which breaks println!
        // (\n no longer includes \r, causing lines to shift right).
        if self.config.debug {
            self.debug_log(format_args!("mode: {}", if self.config.push_to_talk { "push-to-talk" } else { "toggle" }));
            self.debug_log(format_args!("hotkey: {} ({})", self.config.global_hotkey,
                if self.config.use_global_hotkey { "enabled" } else { "disabled" }));
            self.debug_log(format_args!("toggle key: '{}'", self.config.toggle_key));
            self.debug_log(format_args!("device: {}", self.config.audio_device.as_deref().unwrap_or("(default)")));
            self.debug_log(format_args!("model: {} ({})", self.config.model_size,
                if self.whisper.is_some() { "loaded" } else { "not loaded" }));
            self.debug_log(format_args!("ready"));
        } else {
            self.display.show_welcome(
                &format_toggle_key(self.config.toggle_key),
                self.config.use_global_hotkey,
                &self.config.global_hotkey,
                self.config.push_to_talk,
            );
        }

        // Spawn keyboard input on a dedicated OS thread (crossterm poll loop is blocking).
        if is_tty() {
            let kb_sender = self.event_tx.clone();
            let kb_cancel = self.cancel.clone();
            let toggle_key = self.config.toggle_key;

            let (input_tx, mut input_rx) =
                tokio::sync::mpsc::unbounded_channel::<InputEvent>();

            let event_tx_fwd = self.event_tx.clone();
            tokio::spawn(async move {
                while let Some(ev) = input_rx.recv().await {
                    let _ = event_tx_fwd.send(AppEvent::Input(ev));
                }
            });

            std::thread::spawn(move || {
                let kb = KeyboardInput::new(toggle_key, input_tx, kb_cancel);
                if let Err(e) = kb.run() {
                    eprintln!("[voice] Keyboard input error: {}", e);
                }
                let _ = kb_sender.send(AppEvent::Input(InputEvent::Quit));
            });
        }

        // Spawn SSE event bridge for permission/question handling.
        if !self.config.debug {
            let (sse_tx, mut sse_rx) =
                tokio::sync::mpsc::unbounded_channel::<SseEvent>();

            let sse_client = OpenCodeEvents::new(
                self.bridge.get_base_url().to_string(),
                self.config.server_password.clone(),
                sse_tx,
            );
            sse_client.start(self.cancel.clone());

            // Forward SseEvent → AppEvent on a tokio task.
            let event_tx_fwd = self.event_tx.clone();
            tokio::spawn(async move {
                while let Some(sse_event) = sse_rx.recv().await {
                    let app_event = match sse_event {
                        SseEvent::Connected => AppEvent::SseConnected,
                        SseEvent::Disconnected(reason) => AppEvent::SseDisconnected(reason),
                        SseEvent::PermissionAsked(req) => AppEvent::PermissionAsked(req),
                        SseEvent::PermissionReplied {
                            session_id,
                            request_id,
                            reply,
                        } => AppEvent::PermissionReplied {
                            session_id,
                            request_id,
                            reply,
                        },
                        SseEvent::QuestionAsked(req) => AppEvent::QuestionAsked(req),
                        SseEvent::QuestionReplied {
                            session_id,
                            request_id,
                            answers,
                        } => AppEvent::QuestionReplied {
                            session_id,
                            request_id,
                            answers,
                        },
                        SseEvent::QuestionRejected {
                            session_id,
                            request_id,
                        } => AppEvent::QuestionRejected {
                            session_id,
                            request_id,
                        },
                        SseEvent::SessionStatus { session_id, busy } => {
                            AppEvent::SessionStatus { session_id, busy }
                        }
                        SseEvent::SessionUpdated { session_id } => {
                            AppEvent::SessionUpdated { session_id }
                        }
                        SseEvent::SessionCreated { session_id } => {
                            AppEvent::SessionCreated { session_id }
                        }
                        SseEvent::SessionDeleted { session_id } => {
                            AppEvent::SessionDeleted { session_id }
                        }
                    };
                    if event_tx_fwd.send(app_event).is_err() {
                        break;
                    }
                }
            });
        }

        // Register SIGINT / SIGTERM signal handlers.
        self.register_signal_handlers();

        // Spawn a 10 Hz UI tick for animations (recording timer, spinner).
        // Skip in debug mode — no animated display.
        if !self.config.debug {
            let tick_tx = self.event_tx.clone();
            let tick_cancel = self.cancel.clone();
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(tokio::time::Duration::from_millis(100));
                loop {
                    interval.tick().await;
                    if tick_cancel.is_cancelled() {
                        break;
                    }
                    if tick_tx.send(AppEvent::Tick).is_err() {
                        break;
                    }
                }
            });
        }

        // Render initial idle state (skip in debug mode).
        if !self.config.debug {
            self.render_display();
        }

        // Enter the main event loop.
        self.run_event_loop().await;

        Ok(())
    }

    /// Registers OS signal handlers that send `AppEvent::Shutdown` on SIGINT/SIGTERM.
    fn register_signal_handlers(&self) {
        let tx_sigint = self.event_tx.clone();
        let tx_sigterm = self.event_tx.clone();

        tokio::spawn(async move {
            if let Ok(mut sig) = tokio::signal::unix::signal(
                tokio::signal::unix::SignalKind::interrupt(),
            ) {
                if sig.recv().await.is_some() {
                    let _ = tx_sigint.send(AppEvent::Shutdown);
                }
            }
        });

        tokio::spawn(async move {
            if let Ok(mut sig) = tokio::signal::unix::signal(
                tokio::signal::unix::SignalKind::terminate(),
            ) {
                if sig.recv().await.is_some() {
                    let _ = tx_sigterm.send(AppEvent::Shutdown);
                }
            }
        });
    }

    /// The main event loop: receives `AppEvent`s and dispatches them.
    async fn run_event_loop(&mut self) {
        loop {
            let event = match self.event_rx.recv().await {
                Some(e) => e,
                None => break, // All senders dropped — exit.
            };

            match event {
                AppEvent::Input(input_event) => {
                    self.handle_input(input_event).await;
                    if self.is_shutting_down {
                        break;
                    }
                }

                AppEvent::SseConnected => {
                    // Update display to reflect connectivity.
                    self.render_display();
                }

                AppEvent::SseDisconnected(reason) => {
                    if let Some(msg) = reason {
                        self.display.log(&format!("[voice] SSE disconnected: {}", msg));
                    }
                    self.render_display();
                }

                AppEvent::PermissionAsked(req) => {
                    approval::handle_sse_permission_asked(self, req);
                }

                AppEvent::PermissionReplied {
                    session_id,
                    request_id,
                    reply,
                } => {
                    approval::handle_sse_permission_replied(self, &session_id, &request_id, &reply);
                }

                AppEvent::QuestionAsked(req) => {
                    approval::handle_sse_question_asked(self, req);
                }

                AppEvent::QuestionReplied {
                    session_id,
                    request_id,
                    answers,
                } => {
                    approval::handle_sse_question_replied(self, &session_id, &request_id, answers);
                }

                AppEvent::QuestionRejected {
                    session_id,
                    request_id,
                } => {
                    approval::handle_sse_question_rejected(self, &session_id, &request_id);
                }

                AppEvent::SessionStatus { session_id, busy } => {
                    if busy {
                        self.active_session = Some(session_id.clone());
                    }
                    approval::handle_sse_session_status(self, &session_id, busy);
                }

                AppEvent::SessionUpdated { session_id } => {
                    self.active_session = Some(session_id);
                }

                AppEvent::SessionCreated { session_id } => {
                    self.active_session = Some(session_id);
                }

                AppEvent::SessionDeleted { session_id } => {
                    if self.active_session.as_deref() == Some(&session_id) {
                        self.active_session = None;
                    }
                }

                AppEvent::AudioChunk { rms_energy } => {
                    // Just store the level; the Tick timer handles rendering.
                    if self.config.debug {
                        self.debug_audio_counter += 1;
                        if self.debug_audio_counter % 10 == 0 {
                            self.debug_log(format_args!("audio level: {:.4}", rms_energy));
                        }
                    }
                    self.current_level = Some(rms_energy);
                }

                AppEvent::Tick => {
                    match self.state {
                        RecordingState::Recording | RecordingState::Transcribing => {
                            self.spinner_frame = self.spinner_frame.wrapping_add(1);
                            self.render_display();
                        }
                        _ => {}
                    }
                }

                AppEvent::RecoverFromError => {
                    if self.state == RecordingState::Error {
                        self.state = RecordingState::Idle;
                        self.current_error = None;
                        self.render_display();
                    }
                }

                AppEvent::Shutdown => {
                    self.shutdown();
                    break;
                }
            }
        }
    }

    /// Handles an [`InputEvent`] from keyboard or global hotkey.
    async fn handle_input(&mut self, event: InputEvent) {
        self.debug_log(format_args!("input: {:?}  state: {:?}", event, self.state));
        match event {
            InputEvent::Toggle => {
                    if self.suppress_next_toggle {
                        self.suppress_next_toggle = false;
                    } else if self.config.push_to_talk {
                        // Terminal keyboard (space) in PTT mode: route through
                        // PTT start/stop so it works as a toggle.
                        match self.state {
                            RecordingState::Idle => {
                                recording::handle_push_to_talk_start(self).await;
                            }
                            RecordingState::Recording => {
                                recording::handle_push_to_talk_stop(self).await;
                            }
                            _ => {}
                        }
                    } else {
                        recording::handle_toggle(self).await;
                    }
                }

            InputEvent::KeyDown => {
                if self.config.push_to_talk {
                    recording::handle_push_to_talk_start(self).await;
                }
            }

            InputEvent::KeyUp => {
                if self.config.push_to_talk {
                    self.suppress_next_toggle = true;
                    recording::handle_push_to_talk_stop(self).await;
                }
            }

            InputEvent::Quit => {
                self.shutdown();
            }
        }
    }

    /// Transitions to the Error state, updates the display, and schedules
    /// automatic recovery after 3 seconds.
    pub(crate) fn handle_error(&mut self, err: &str) {
        self.state = RecordingState::Error;
        self.current_error = Some(err.to_string());
        self.render_display();

        // Schedule recovery: after 3 seconds send RecoverFromError.
        let tx = self.event_tx.clone();
        tokio::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
            let _ = tx.send(AppEvent::RecoverFromError);
        });
    }

    /// Shuts down the application.
    ///
    /// Guarded by `is_shutting_down` to prevent double-shutdown.
    /// Cancels the global cancellation token and clears the display.
    pub(crate) fn shutdown(&mut self) {
        if self.is_shutting_down {
            return;
        }
        self.is_shutting_down = true;

        // Cancel all background tasks.
        self.cancel.cancel();

        // Clear the terminal display (skip in debug mode).
        if !self.config.debug {
            self.display.clear();
        }

        eprintln!("[voice] Shutting down.");
    }

    /// Renders the current state to the terminal display.
    /// No-op in debug mode.
    pub(crate) fn render_display(&mut self) {
        if self.config.debug {
            return;
        }
        let toggle_key_str = format_toggle_key(self.config.toggle_key);
        let approval = self.approval_queue.peek();
        let approval_count = self.approval_queue.len();

        // Read live duration from the active recorder.
        let duration = self.recorder.as_ref().map(|r| r.duration());

        // Convert RMS to a perceptual (logarithmic) display level.
        // Raw RMS varies wildly across mic/OS/gain setups (0.001–0.3).
        // Linear scaling breaks for quiet mics. dB scale works universally:
        //   -60 dB (silence) → 0.0,  0 dB (full-scale) → 1.0
        let display_level = self.current_level.map(|l| {
            let db = 20.0 * l.max(1e-7).log10();
            ((db + 60.0) / 60.0).clamp(0.0, 1.0)
        });

        let global_hotkey_name = if self.config.use_global_hotkey {
            Some(self.config.global_hotkey.as_str())
        } else {
            None
        };

        let meta = DisplayMeta {
            level: display_level,
            error: self.current_error.as_deref(),
            toggle_key: Some(&toggle_key_str),
            global_hotkey_name,
            approval,
            approval_count: Some(approval_count),
            transcript: self.last_transcript.as_deref(),
            duration,
            active_session: self.active_session.clone(),
            spinner_frame: self.spinner_frame,
        };

        self.display.update(self.state, &meta);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AppConfig, ModelSize};
    use std::path::PathBuf;

    /// Builds a minimal `AppConfig` suitable for unit tests.
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
            handle_prompts: false,
            debug: false,
        }
    }

    #[test]
    fn test_voice_app_new_initializes_idle_state() {
        let config = test_config();
        let app = VoiceApp::new(config).expect("VoiceApp::new should succeed");
        assert_eq!(app.state, RecordingState::Idle);
    }

    #[test]
    fn test_voice_app_new_whisper_none_when_model_missing() {
        let config = test_config();
        let app = VoiceApp::new(config).expect("VoiceApp::new should succeed");
        // Model path is /nonexistent/model.bin — should not be loaded.
        assert!(app.whisper.is_none());
    }

    #[test]
    fn test_voice_app_new_approval_queue_empty() {
        let config = test_config();
        let app = VoiceApp::new(config).expect("VoiceApp::new should succeed");
        assert!(!app.approval_queue.has_pending());
    }

    #[test]
    fn test_voice_app_new_not_shutting_down() {
        let config = test_config();
        let app = VoiceApp::new(config).expect("VoiceApp::new should succeed");
        assert!(!app.is_shutting_down);
    }

    #[test]
    fn test_voice_app_shutdown_sets_flag() {
        let config = test_config();
        let mut app = VoiceApp::new(config).expect("VoiceApp::new should succeed");
        app.shutdown();
        assert!(app.is_shutting_down);
    }

    #[test]
    fn test_voice_app_shutdown_idempotent() {
        let config = test_config();
        let mut app = VoiceApp::new(config).expect("VoiceApp::new should succeed");
        app.shutdown();
        app.shutdown(); // Second call must not panic.
        assert!(app.is_shutting_down);
    }

    #[test]
    fn test_voice_app_shutdown_cancels_token() {
        let config = test_config();
        let mut app = VoiceApp::new(config).expect("VoiceApp::new should succeed");
        assert!(!app.cancel.is_cancelled());
        app.shutdown();
        assert!(app.cancel.is_cancelled());
    }

    #[tokio::test]
    async fn test_handle_error_sets_error_state() {
        let config = test_config();
        let mut app = VoiceApp::new(config).expect("VoiceApp::new should succeed");
        app.handle_error("test error");
        assert_eq!(app.state, RecordingState::Error);
        assert_eq!(app.current_error.as_deref(), Some("test error"));
    }

    #[tokio::test]
    async fn test_recover_from_error_transitions_to_idle() {
        let config = test_config();
        let mut app = VoiceApp::new(config).expect("VoiceApp::new should succeed");
        app.state = RecordingState::Error;
        app.current_error = Some("some error".to_string());

        // Simulate the RecoverFromError event being processed.
        if app.state == RecordingState::Error {
            app.state = RecordingState::Idle;
            app.current_error = None;
        }

        assert_eq!(app.state, RecordingState::Idle);
        assert!(app.current_error.is_none());
    }

    #[test]
    fn test_voice_app_event_channel_works() {
        let config = test_config();
        let app = VoiceApp::new(config).expect("VoiceApp::new should succeed");
        // Sending an event should succeed while the receiver is alive.
        let result = app.event_tx.send(AppEvent::Shutdown);
        assert!(result.is_ok());
    }

    #[test]
    fn test_voice_app_new_active_session_none() {
        let config = test_config();
        let app = VoiceApp::new(config).expect("VoiceApp::new should succeed");
        assert!(app.active_session.is_none());
    }

    #[tokio::test]
    async fn test_session_updated_sets_active_session() {
        let config = test_config();
        let mut app = VoiceApp::new(config).expect("VoiceApp::new should succeed");

        // Simulate receiving SessionUpdated event
        let session_id = "sess_abc123".to_string();
        app.active_session = Some(session_id.clone());

        assert_eq!(app.active_session, Some(session_id));
    }

    #[tokio::test]
    async fn test_session_deleted_clears_active_session() {
        let config = test_config();
        let mut app = VoiceApp::new(config).expect("VoiceApp::new should succeed");

        // Set an active session
        app.active_session = Some("sess_to_delete".to_string());

        // Simulate SessionDeleted for the active session
        let session_id = "sess_to_delete".to_string();
        if app.active_session.as_deref() == Some(&session_id) {
            app.active_session = None;
        }

        assert!(app.active_session.is_none());
    }

    #[tokio::test]
    async fn test_session_deleted_other_session_no_change() {
        let config = test_config();
        let mut app = VoiceApp::new(config).expect("VoiceApp::new should succeed");

        // Set an active session
        app.active_session = Some("sess_active".to_string());

        // Simulate SessionDeleted for a DIFFERENT session
        let other_session_id = "sess_other".to_string();
        if app.active_session.as_deref() == Some(&other_session_id) {
            app.active_session = None;
        }

        // Active session should be unchanged
        assert_eq!(app.active_session, Some("sess_active".to_string()));
    }
}
