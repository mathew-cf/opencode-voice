//! whisper-rs transcription engine: in-process speech-to-text.
//!
//! Replaces the whisper-cli subprocess with native whisper-rs bindings.

use anyhow::{Context, Result};
use std::path::Path;

/// Result of a transcription operation.
pub struct TranscriptionResult {
    pub text: String,
}

/// Checks if a model file is valid (exists and is > 1MB).
pub fn is_model_valid(path: &Path) -> bool {
    path.exists()
        && path
            .metadata()
            .map(|m| m.len() > 1_000_000)
            .unwrap_or(false)
}

/// In-process whisper transcription engine.
pub struct WhisperEngine {
    ctx: whisper_rs::WhisperContext,
}

impl WhisperEngine {
    /// Loads a GGML model file and creates a WhisperEngine.
    pub fn new(model_path: &Path) -> Result<Self> {
        if !model_path.exists() {
            anyhow::bail!(
                "Whisper model not found at {}. Run 'opencode-voice setup' to download it.",
                model_path.display()
            );
        }

        let path_str = model_path
            .to_str()
            .context("Model path contains invalid UTF-8")?;

        // Suppress whisper.cpp's verbose C-level logging during model load.
        // whisper-rs 0.13.2 doesn't expose `no_prints` in WhisperContextParameters,
        // so we install a no-op log callback via the sys crate.
        suppress_whisper_logging();

        let ctx = whisper_rs::WhisperContext::new_with_params(
            path_str,
            whisper_rs::WhisperContextParameters::default(),
        )
        .map_err(|e| anyhow::anyhow!("Failed to load whisper model: {:?}", e))?;

        Ok(WhisperEngine { ctx })
    }

    /// Transcribes a WAV file and returns the text.
    ///
    /// Note: This is CPU-bound and blocking. Call via `tokio::task::spawn_blocking`
    /// in an async context to avoid blocking the async runtime.
    pub fn transcribe(&self, wav_path: &Path) -> Result<TranscriptionResult> {
        // Read WAV file
        let mut reader = hound::WavReader::open(wav_path)
            .with_context(|| format!("Failed to open WAV file: {}", wav_path.display()))?;

        // Convert i16 samples to f32 (whisper-rs expects f32 in range [-1.0, 1.0])
        let samples: Vec<f32> = reader
            .samples::<i16>()
            .filter_map(|s| s.ok())
            .map(|s| s as f32 / 32768.0)
            .collect();

        if samples.is_empty() {
            return Ok(TranscriptionResult {
                text: String::new(),
            });
        }

        // Set up whisper params: no timestamps, no progress output
        let mut params =
            whisper_rs::FullParams::new(whisper_rs::SamplingStrategy::Greedy { best_of: 1 });
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_no_timestamps(true);
        params.set_single_segment(false);

        // Run transcription
        let mut state = self
            .ctx
            .create_state()
            .map_err(|e| anyhow::anyhow!("Failed to create whisper state: {:?}", e))?;

        state
            .full(params, &samples)
            .map_err(|e| anyhow::anyhow!("Whisper transcription failed: {:?}", e))?;

        // Collect segments
        let num_segments = state
            .full_n_segments()
            .map_err(|e| anyhow::anyhow!("Failed to get segment count: {:?}", e))?;

        let mut text_parts: Vec<String> = Vec::new();
        for i in 0..num_segments {
            if let Ok(segment) = state.full_get_segment_text(i) {
                text_parts.push(segment);
            }
        }

        let raw_text = text_parts.join(" ");

        // Strip timestamp brackets like [HH:MM:SS.mmm --> HH:MM:SS.mmm]
        let clean_text = strip_timestamps(&raw_text);
        // Filter out Whisper hallucination artifacts (e.g. "[BLANK_AUDIO]")
        let clean_text = strip_hallucinations(&clean_text);
        let final_text = clean_text.trim().to_string();

        Ok(TranscriptionResult { text: final_text })
    }
}

/// Installs a no-op log callback to suppress whisper.cpp's C-level stderr output.
///
/// This must be called before `WhisperContext::new_with_params` to prevent
/// the verbose model-loading messages from cluttering the terminal.
fn suppress_whisper_logging() {
    unsafe {
        // A C-compatible no-op callback that discards all whisper log messages.
        unsafe extern "C" fn noop_log(
            _level: whisper_rs::whisper_rs_sys::ggml_log_level,
            _text: *const std::ffi::c_char,
            _user_data: *mut std::ffi::c_void,
        ) {
        }
        whisper_rs::whisper_rs_sys::whisper_log_set(Some(noop_log), std::ptr::null_mut());
        whisper_rs::whisper_rs_sys::ggml_log_set(Some(noop_log), std::ptr::null_mut());
    }
}

/// Known Whisper hallucination phrases that should be treated as silence.
///
/// These are bracketed tags or repeated filler phrases that Whisper emits
/// when the audio contains silence, noise, or non-speech content.
const WHISPER_HALLUCINATIONS: &[&str] = &[
    "[BLANK_AUDIO]",
    "[NO_SPEECH]",
    "(blank audio)",
    "(no speech)",
    "[silence]",
    "(silence)",
];

/// Removes known Whisper hallucination artifacts from transcribed text.
///
/// If the entire text (after removal) is empty, returns an empty string
/// so the caller treats it the same as silence.
fn strip_hallucinations(text: &str) -> String {
    let mut result = text.to_string();
    for pattern in WHISPER_HALLUCINATIONS {
        // Case-insensitive removal
        while let Some(pos) = result.to_lowercase().find(&pattern.to_lowercase()) {
            result = format!("{}{}", &result[..pos], &result[pos + pattern.len()..]);
        }
    }
    result
}

/// Strips whisper timestamp annotations from transcribed text.
///
/// Example: "[00:00:00.000 --> 00:00:05.000] Hello world" → "Hello world"
fn strip_timestamps(text: &str) -> String {
    // Remove patterns like [HH:MM:SS.mmm --> HH:MM:SS.mmm]
    let mut result = text.to_string();
    while let Some(start) = result.find('[') {
        if let Some(end) = result[start..].find(']') {
            let bracket_content = &result[start + 1..start + end];
            // Only remove if it looks like a timestamp (contains "-->")
            if bracket_content.contains("-->") {
                result = format!("{}{}", &result[..start], &result[start + end + 1..]);
            } else {
                break;
            }
        } else {
            break;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_model_valid_nonexistent() {
        assert!(!is_model_valid(Path::new("/nonexistent/path/model.bin")));
    }

    #[test]
    fn test_is_model_valid_small_file() {
        // Create a tiny temp file (< 1MB)
        let tmp = std::env::temp_dir().join("test-tiny.bin");
        std::fs::write(&tmp, b"tiny").unwrap();
        assert!(!is_model_valid(&tmp));
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn test_strip_timestamps_with_arrow() {
        let input = "[00:00:00.000 --> 00:00:05.000]  Hello world";
        let result = strip_timestamps(input);
        assert!(!result.contains("-->"));
        assert!(result.contains("Hello world"));
    }

    #[test]
    fn test_strip_timestamps_no_timestamps() {
        let input = "Hello world";
        assert_eq!(strip_timestamps(input), "Hello world");
    }

    #[test]
    fn test_strip_timestamps_preserves_non_timestamp_brackets() {
        let input = "Hello [world]";
        let result = strip_timestamps(input);
        assert!(result.contains("[world]")); // Non-timestamp bracket preserved
    }

    #[test]
    fn test_strip_hallucinations_blank_audio() {
        assert_eq!(strip_hallucinations("[BLANK_AUDIO]").trim(), "");
    }

    #[test]
    fn test_strip_hallucinations_case_insensitive() {
        assert_eq!(strip_hallucinations("[blank_audio]").trim(), "");
        assert_eq!(strip_hallucinations("[Blank_Audio]").trim(), "");
    }

    #[test]
    fn test_strip_hallucinations_preserves_real_text() {
        assert_eq!(strip_hallucinations("hello world"), "hello world");
    }

    #[test]
    fn test_strip_hallucinations_mixed() {
        let result = strip_hallucinations("[BLANK_AUDIO] hello [BLANK_AUDIO]");
        assert_eq!(result.trim(), "hello");
    }
}
