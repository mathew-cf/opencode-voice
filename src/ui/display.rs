//! ANSI terminal display renderer for the voice mode UI.

use crossterm::{
    cursor,
    terminal::{Clear, ClearType},
    QueueableCommand,
};
use std::io::{self, Write};

use crate::approval::types::PendingApproval;
use crate::state::RecordingState;

/// Braille-dot spinner frames for animated states.
const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Optional metadata for the display renderer.
#[derive(Default)]
pub struct DisplayMeta<'a> {
    pub duration: Option<f64>,
    pub level: Option<f32>,
    pub transcript: Option<&'a str>,
    pub error: Option<&'a str>,
    pub toggle_key: Option<&'a str>,
    /// When a global hotkey is active, this carries the hotkey name
    /// (e.g. "right_option") so the status line shows the actual key.
    pub global_hotkey_name: Option<&'a str>,
    pub approval: Option<&'a PendingApproval>,
    pub approval_count: Option<usize>,
    /// Monotonically increasing frame counter for spinner animation.
    pub spinner_frame: usize,
}

/// Renders an ASCII level bar like `[||||    ]`.
pub fn render_level(level: f32, width: usize) -> String {
    let filled = ((level * width as f32).round() as usize).min(width);
    let empty = width - filled;
    format!("[{}{}]", "|".repeat(filled), " ".repeat(empty))
}

/// In-place terminal renderer.
///
/// Uses absolute cursor positioning (`origin_row`) so that interleaved
/// stderr output (e.g. from `eprintln!`) cannot corrupt the display area.
pub struct Display {
    line_count: u16,
}

impl Display {
    pub fn new() -> Self {
        Display { line_count: 0 }
    }

    /// Erases previously rendered lines and renders the new state **in place**.
    ///
    /// Uses only relative cursor movement (`MoveUp`) so it works with or
    /// without raw mode and is immune to `cursor::position()` hangs.
    ///
    /// The cursor is left at the end of the last rendered line (no trailing
    /// newline) so that the next `update` can move back up exactly
    /// `line_count - 1` lines to reach the first rendered line.
    pub fn update(&mut self, state: RecordingState, meta: &DisplayMeta) {
        let mut stdout = io::stdout();

        // Move cursor back to the start of the first line we rendered last
        // time.  After the previous render the cursor sits at the end of the
        // last content line (no trailing \n), so we need to go up
        // (line_count - 1) lines, then to column 0.
        if self.line_count > 1 {
            let _ = stdout.queue(cursor::MoveUp(self.line_count - 1));
        }
        if self.line_count > 0 {
            let _ = stdout.queue(cursor::MoveToColumn(0));
            let _ = stdout.queue(Clear(ClearType::FromCursorDown));
        }

        // Render new state
        let lines = self.render_state(state, meta);
        self.line_count = lines.len() as u16;

        for (i, line) in lines.iter().enumerate() {
            let _ = stdout.queue(crossterm::style::Print(line));
            // Newline between lines, but NOT after the last one — keeps the
            // cursor on the content so the next update can overwrite cleanly.
            if i + 1 < lines.len() {
                let _ = stdout.queue(crossterm::style::Print("\r\n"));
            }
        }
        let _ = stdout.flush();
    }

    /// Erases all rendered lines and resets.
    pub fn clear(&mut self) {
        let mut stdout = io::stdout();
        if self.line_count > 1 {
            let _ = stdout.queue(cursor::MoveUp(self.line_count - 1));
        }
        if self.line_count > 0 {
            let _ = stdout.queue(cursor::MoveToColumn(0));
            let _ = stdout.queue(Clear(ClearType::FromCursorDown));
        }
        self.line_count = 0;
        let _ = stdout.flush();
    }

    /// Prints a log message **above** the display area.
    ///
    /// Clears the current display, writes `msg` to stdout on its own line,
    /// then resets `line_count` so the next `update()` renders cleanly below.
    /// All output goes through stdout to avoid cursor-tracking issues with
    /// stderr interleaving.
    pub fn log(&mut self, msg: &str) {
        self.clear();
        let mut stdout = io::stdout();
        let _ = stdout.queue(crossterm::style::Print(msg));
        let _ = stdout.queue(crossterm::style::Print("\r\n"));
        let _ = stdout.flush();
        // line_count is already 0 from clear(), so the next update()
        // will render starting at the current cursor position.
    }

    /// Prints the welcome banner. NOT tracked in line_count.
    pub fn show_welcome(
        &self,
        toggle_key: &str,
        global_hotkey: bool,
        global_hotkey_name: &str,
        push_to_talk: bool,
    ) {
        println!("\x1b[1;36m━━━ OpenCode Voice Mode ━━━\x1b[0m");
        if push_to_talk && global_hotkey {
            println!("  Hold [{}] to record (global hotkey)", global_hotkey_name);
            println!("  Press [{}] to toggle recording (terminal)", toggle_key);
        } else {
            println!("  Press [{}] to toggle recording", toggle_key);
        }
        println!("  Press [q] or Ctrl+C to quit");
        println!();
    }

    fn render_state(&self, state: RecordingState, meta: &DisplayMeta) -> Vec<String> {
        match state {
            RecordingState::Idle => {
                let key_hint = meta
                    .global_hotkey_name
                    .or(meta.toggle_key)
                    .map(|k| format!(" [{}]", k))
                    .unwrap_or_default();
                if let Some(transcript) = meta.transcript {
                    let preview: String = transcript.chars().take(60).collect();
                    let ellipsis = if transcript.len() > 60 { "..." } else { "" };
                    vec![
                        format!("\x1b[32m● Ready{}\x1b[0m", key_hint),
                        format!("  Sent: {}{}", preview, ellipsis),
                    ]
                } else {
                    vec![format!(
                        "\x1b[32m● Ready{} — Press to speak\x1b[0m",
                        key_hint
                    )]
                }
            }
            RecordingState::Recording => {
                let duration = meta.duration.unwrap_or(0.0);
                let level_bar = meta
                    .level
                    .map(|l| format!(" {}", render_level(l, 8)))
                    .unwrap_or_default();
                vec![format!(
                    "\x1b[31m● REC{} {:.1}s\x1b[0m",
                    level_bar, duration
                )]
            }
            RecordingState::Transcribing => {
                let frame = SPINNER_FRAMES[meta.spinner_frame % SPINNER_FRAMES.len()];
                vec![format!("\x1b[33m{} Transcribing...\x1b[0m", frame)]
            }
            RecordingState::Injecting => {
                vec!["\x1b[36m→ Sending to OpenCode...\x1b[0m".to_string()]
            }
            RecordingState::ApprovalPending => {
                let count = meta.approval_count.unwrap_or(0);
                let count_str = if count > 1 {
                    format!(" (+{} more)", count - 1)
                } else {
                    String::new()
                };

                if let Some(approval) = meta.approval {
                    match approval {
                        PendingApproval::Permission(req) => {
                            let detail = format_permission_detail(&req.permission, &req.metadata);
                            vec![
                                format!(
                                    "\x1b[35m⚠ Approval needed{}: {} — {}\x1b[0m",
                                    count_str, req.permission, detail
                                ),
                                "  Say: allow/always/reject".to_string(),
                            ]
                        }
                        PendingApproval::Question(req) => {
                            let mut lines = Vec::new();
                            if let Some(q) = req.questions.first() {
                                lines.push(format!("\x1b[35m? {}{}\x1b[0m", q.question, count_str));
                                for (i, opt) in q.options.iter().take(5).enumerate() {
                                    lines.push(format!("  {}. {}", i + 1, opt.label));
                                }
                                lines.push("  Say the option name or number".to_string());
                            } else {
                                lines.push(format!(
                                    "\x1b[35m? Question pending{}\x1b[0m",
                                    count_str
                                ));
                            }
                            lines
                        }
                    }
                } else {
                    vec![format!("\x1b[35m⚠ Approval needed{}\x1b[0m", count_str)]
                }
            }
            RecordingState::Error => {
                let msg = meta.error.unwrap_or("An error occurred");
                vec![
                    format!("\x1b[31m✗ Error: {}\x1b[0m", msg),
                    "  Recovering...".to_string(),
                ]
            }
        }
    }
}

impl Default for Display {
    fn default() -> Self {
        Self::new()
    }
}

/// Formats a human-readable detail for a permission type and metadata.
pub fn format_permission_detail(permission: &str, metadata: &serde_json::Value) -> String {
    match permission {
        "bash" => {
            if let Some(cmd) = metadata.get("command").and_then(|v| v.as_str()) {
                return format!("`{}`", cmd.chars().take(60).collect::<String>());
            }
        }
        "edit" | "write" | "read" => {
            if let Some(path) = metadata.get("path").and_then(|v| v.as_str()) {
                return path.to_string();
            }
        }
        _ => {}
    }
    // Fallback: first string value in metadata
    if let Some(obj) = metadata.as_object() {
        for v in obj.values() {
            if let Some(s) = v.as_str() {
                return s.chars().take(60).collect();
            }
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render_level_empty() {
        assert_eq!(render_level(0.0, 8), "[        ]");
    }

    #[test]
    fn test_render_level_full() {
        assert_eq!(render_level(1.0, 8), "[||||||||]");
    }

    #[test]
    fn test_render_level_half() {
        // 0.5 * 8 = 4.0 → 4 filled, 4 empty
        assert_eq!(render_level(0.5, 8), "[||||    ]");
    }

    #[test]
    fn test_render_level_clamps_above_one() {
        assert_eq!(render_level(2.0, 8), "[||||||||]");
    }

    #[test]
    fn test_render_level_width_zero() {
        assert_eq!(render_level(0.5, 0), "[]");
    }

    #[test]
    fn test_format_permission_detail_bash() {
        let meta = serde_json::json!({ "command": "ls -la" });
        assert_eq!(format_permission_detail("bash", &meta), "`ls -la`");
    }

    #[test]
    fn test_format_permission_detail_edit() {
        let meta = serde_json::json!({ "path": "/tmp/foo.txt" });
        assert_eq!(format_permission_detail("edit", &meta), "/tmp/foo.txt");
    }

    #[test]
    fn test_format_permission_detail_write() {
        let meta = serde_json::json!({ "path": "/tmp/bar.txt" });
        assert_eq!(format_permission_detail("write", &meta), "/tmp/bar.txt");
    }

    #[test]
    fn test_format_permission_detail_read() {
        let meta = serde_json::json!({ "path": "/etc/hosts" });
        assert_eq!(format_permission_detail("read", &meta), "/etc/hosts");
    }

    #[test]
    fn test_format_permission_detail_unknown_fallback() {
        let meta = serde_json::json!({ "target": "some-value" });
        assert_eq!(format_permission_detail("unknown", &meta), "some-value");
    }

    #[test]
    fn test_format_permission_detail_empty_metadata() {
        let meta = serde_json::json!({});
        assert_eq!(format_permission_detail("bash", &meta), "");
    }

    #[test]
    fn test_render_state_idle_no_transcript() {
        let display = Display::new();
        let meta = DisplayMeta {
            toggle_key: Some("space"),
            ..Default::default()
        };
        let lines = display.render_state(RecordingState::Idle, &meta);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("Ready"));
        assert!(lines[0].contains("[space]"));
        assert!(lines[0].contains("Press to speak"));
    }

    #[test]
    fn test_render_state_idle_with_transcript() {
        let display = Display::new();
        let meta = DisplayMeta {
            transcript: Some("hello world"),
            ..Default::default()
        };
        let lines = display.render_state(RecordingState::Idle, &meta);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("Ready"));
        assert!(lines[1].contains("Sent: hello world"));
    }

    #[test]
    fn test_render_state_idle_transcript_truncated() {
        let display = Display::new();
        let long_text = "a".repeat(80);
        let meta = DisplayMeta {
            transcript: Some(&long_text),
            ..Default::default()
        };
        let lines = display.render_state(RecordingState::Idle, &meta);
        assert_eq!(lines.len(), 2);
        assert!(lines[1].contains("..."));
    }

    #[test]
    fn test_render_state_recording() {
        let display = Display::new();
        let meta = DisplayMeta {
            duration: Some(2.5),
            level: Some(0.5),
            ..Default::default()
        };
        let lines = display.render_state(RecordingState::Recording, &meta);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("REC"));
        assert!(lines[0].contains("2.5s"));
        assert!(lines[0].contains("[||||    ]"));
    }

    #[test]
    fn test_render_state_recording_no_level() {
        let display = Display::new();
        let meta = DisplayMeta {
            duration: Some(1.0),
            ..Default::default()
        };
        let lines = display.render_state(RecordingState::Recording, &meta);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("REC"));
        assert!(lines[0].contains("1.0s"));
    }

    #[test]
    fn test_render_state_transcribing() {
        let display = Display::new();
        let meta = DisplayMeta::default();
        let lines = display.render_state(RecordingState::Transcribing, &meta);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("Transcribing"));
    }

    #[test]
    fn test_render_state_injecting() {
        let display = Display::new();
        let meta = DisplayMeta::default();
        let lines = display.render_state(RecordingState::Injecting, &meta);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("Sending to OpenCode"));
    }

    #[test]
    fn test_render_state_error() {
        let display = Display::new();
        let meta = DisplayMeta {
            error: Some("connection failed"),
            ..Default::default()
        };
        let lines = display.render_state(RecordingState::Error, &meta);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("Error: connection failed"));
        assert!(lines[1].contains("Recovering"));
    }

    #[test]
    fn test_render_state_error_default_message() {
        let display = Display::new();
        let meta = DisplayMeta::default();
        let lines = display.render_state(RecordingState::Error, &meta);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("An error occurred"));
    }

    #[test]
    fn test_render_state_approval_pending_no_approval() {
        let display = Display::new();
        let meta = DisplayMeta {
            approval_count: Some(1),
            ..Default::default()
        };
        let lines = display.render_state(RecordingState::ApprovalPending, &meta);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("Approval needed"));
    }

    #[test]
    fn test_render_state_approval_pending_permission() {
        use crate::approval::types::PermissionRequest;

        let display = Display::new();
        let req = PermissionRequest {
            id: "req-1".to_string(),
            permission: "bash".to_string(),
            metadata: serde_json::json!({ "command": "rm -rf /tmp/test" }),
        };
        let approval = PendingApproval::Permission(req);
        let meta = DisplayMeta {
            approval: Some(&approval),
            approval_count: Some(1),
            ..Default::default()
        };
        let lines = display.render_state(RecordingState::ApprovalPending, &meta);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("Approval needed"));
        assert!(lines[0].contains("bash"));
        assert!(lines[0].contains("`rm -rf /tmp/test`"));
        assert!(lines[1].contains("allow/always/reject"));
    }

    #[test]
    fn test_render_state_approval_pending_multiple_count() {
        use crate::approval::types::PermissionRequest;

        let display = Display::new();
        let req = PermissionRequest {
            id: "req-1".to_string(),
            permission: "edit".to_string(),
            metadata: serde_json::json!({ "path": "/tmp/file.txt" }),
        };
        let approval = PendingApproval::Permission(req);
        let meta = DisplayMeta {
            approval: Some(&approval),
            approval_count: Some(3),
            ..Default::default()
        };
        let lines = display.render_state(RecordingState::ApprovalPending, &meta);
        assert!(lines[0].contains("+2 more"));
    }

    #[test]
    fn test_render_state_approval_pending_question() {
        use crate::approval::types::{QuestionInfo, QuestionOption, QuestionRequest};

        let display = Display::new();
        let req = QuestionRequest {
            id: "q-1".to_string(),
            questions: vec![QuestionInfo {
                question: "Which approach?".to_string(),
                options: vec![
                    QuestionOption {
                        label: "Option A".to_string(),
                    },
                    QuestionOption {
                        label: "Option B".to_string(),
                    },
                ],
                custom: true,
            }],
        };
        let approval = PendingApproval::Question(req);
        let meta = DisplayMeta {
            approval: Some(&approval),
            approval_count: Some(1),
            ..Default::default()
        };
        let lines = display.render_state(RecordingState::ApprovalPending, &meta);
        assert!(lines[0].contains("Which approach?"));
        assert!(lines[1].contains("1. Option A"));
        assert!(lines[2].contains("2. Option B"));
        assert!(lines
            .last()
            .unwrap()
            .contains("Say the option name or number"));
    }

    #[test]
    fn test_render_state_approval_pending_question_empty() {
        use crate::approval::types::QuestionRequest;

        let display = Display::new();
        let req = QuestionRequest {
            id: "q-1".to_string(),
            questions: vec![],
        };
        let approval = PendingApproval::Question(req);
        let meta = DisplayMeta {
            approval: Some(&approval),
            approval_count: Some(1),
            ..Default::default()
        };
        let lines = display.render_state(RecordingState::ApprovalPending, &meta);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("Question pending"));
    }

    #[test]
    fn test_display_new_initial_state() {
        let display = Display::new();
        assert_eq!(display.line_count, 0);
    }

    #[test]
    fn test_display_default() {
        let display = Display::default();
        assert_eq!(display.line_count, 0);
    }

    #[test]
    fn test_all_states_produce_output() {
        let display = Display::new();
        let meta = DisplayMeta::default();

        let states = [
            RecordingState::Idle,
            RecordingState::Recording,
            RecordingState::Transcribing,
            RecordingState::Injecting,
            RecordingState::ApprovalPending,
            RecordingState::Error,
        ];

        for state in states {
            let lines = display.render_state(state, &meta);
            assert!(!lines.is_empty(), "State {:?} produced no output", state);
        }
    }

    #[test]
    fn test_all_states_produce_distinct_output() {
        let display = Display::new();
        let meta = DisplayMeta::default();

        let outputs: Vec<String> = [
            RecordingState::Idle,
            RecordingState::Recording,
            RecordingState::Transcribing,
            RecordingState::Injecting,
            RecordingState::ApprovalPending,
            RecordingState::Error,
        ]
        .iter()
        .map(|&s| display.render_state(s, &meta).join("|"))
        .collect();

        // Each state should produce unique output
        for i in 0..outputs.len() {
            for j in (i + 1)..outputs.len() {
                assert_ne!(
                    outputs[i], outputs[j],
                    "States {} and {} produce identical output",
                    i, j
                );
            }
        }
    }
}
