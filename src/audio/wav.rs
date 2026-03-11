//! WAV file writing and temporary file management.

use anyhow::{Context, Result};
use hound::{WavSpec, WavWriter};
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::audio::AudioConfig;

/// Writes i16 PCM samples to a WAV file.
///
/// Creates a WAV file with the specified audio config (channels, sample_rate, bit_depth).
pub fn write_wav(samples: &[i16], config: &AudioConfig, path: &Path) -> Result<()> {
    let spec = WavSpec {
        channels: config.channels,
        sample_rate: config.sample_rate,
        bits_per_sample: config.bit_depth,
        sample_format: hound::SampleFormat::Int,
    };

    let mut writer = WavWriter::create(path, spec)
        .with_context(|| format!("Failed to create WAV file at {}", path.display()))?;

    for &sample in samples {
        writer
            .write_sample(sample)
            .context("Failed to write audio sample")?;
    }

    writer.finalize().context("Failed to finalize WAV file")?;

    Ok(())
}

/// Returns a path in the system temp directory with a UUID filename.
///
/// Example: `/tmp/opencode-voice-550e8400-e29b-41d4-a716-446655440000.wav`
pub fn create_temp_wav_path() -> PathBuf {
    let filename = format!("opencode-voice-{}.wav", Uuid::new_v4());
    std::env::temp_dir().join(filename)
}

/// RAII wrapper for a temporary WAV file — deletes on drop.
///
/// The file is created lazily when `write` is called. On drop, the file is
/// deleted (errors are silently ignored). Use `into_path()` to take ownership
/// of the path without triggering deletion.
pub struct TempWav {
    path: PathBuf,
}

impl TempWav {
    /// Creates a new TempWav with a fresh temp path (no file created yet).
    pub fn new() -> Self {
        TempWav {
            path: create_temp_wav_path(),
        }
    }

    /// Writes audio samples to the WAV file.
    pub fn write(&self, samples: &[i16], config: &AudioConfig) -> Result<()> {
        write_wav(samples, config, &self.path)
    }

    /// Consumes the TempWav and returns the path WITHOUT deleting the file.
    ///
    /// The caller takes ownership of the file and is responsible for cleanup.
    pub fn into_path(self) -> PathBuf {
        let path = self.path.clone();
        std::mem::forget(self); // Prevent Drop from deleting the file
        path
    }
}

impl Drop for TempWav {
    fn drop(&mut self) {
        // Silently ignore if file doesn't exist or deletion fails
        let _ = std::fs::remove_file(&self.path);
    }
}

impl Default for TempWav {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::default_audio_config;

    #[test]
    fn test_create_temp_wav_path_has_uuid() {
        let path = create_temp_wav_path();
        let filename = path.file_name().unwrap().to_str().unwrap();
        assert!(filename.starts_with("opencode-voice-"));
        assert!(filename.ends_with(".wav"));
    }

    #[test]
    fn test_create_temp_wav_path_in_temp_dir() {
        let path = create_temp_wav_path();
        assert!(path.starts_with(std::env::temp_dir()));
    }

    #[test]
    fn test_write_and_delete() {
        let config = default_audio_config();
        let samples: Vec<i16> = vec![0i16; 1000]; // 1000 silent samples
        let wav = TempWav::new();
        let path = wav.path.to_path_buf();

        // File doesn't exist yet
        assert!(!path.exists(), "File should not exist before write");

        // Write samples
        wav.write(&samples, &config).expect("write should succeed");
        assert!(path.exists(), "File should exist after write");

        // Drop should delete the file
        drop(wav);
        assert!(!path.exists(), "File should be deleted after drop");
    }

    #[test]
    fn test_drop_no_panic_when_file_missing() {
        // TempWav drop should not panic even if the file was never written
        let wav = TempWav::new();
        // Don't write anything — just drop
        drop(wav); // Should not panic
    }

    #[test]
    fn test_write_wav_creates_valid_file() {
        let config = default_audio_config();
        let samples: Vec<i16> = (0..160).map(|i| i as i16 * 100).collect(); // 10ms at 16kHz
        let path = create_temp_wav_path();

        write_wav(&samples, &config, &path).expect("write_wav should succeed");
        assert!(path.exists());

        // Read back and verify
        let reader = hound::WavReader::open(&path).expect("should be readable");
        let spec = reader.spec();
        assert_eq!(spec.channels, 1);
        assert_eq!(spec.sample_rate, 16000);
        assert_eq!(spec.bits_per_sample, 16);

        // Cleanup
        let _ = std::fs::remove_file(&path);
    }
}
