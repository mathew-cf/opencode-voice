//! Audio types and configuration constants.

pub mod capture;
pub mod wav;

/// Audio capture configuration.
#[derive(Debug, Clone)]
pub struct AudioConfig {
    pub device: Option<String>,
    pub sample_rate: u32,
    pub channels: u16,
    pub bit_depth: u16,
}

/// Returns the default audio configuration: 16kHz mono 16-bit PCM (optimal for whisper.cpp).
pub fn default_audio_config() -> AudioConfig {
    AudioConfig {
        device: None,
        sample_rate: 16_000,
        channels: 1,
        bit_depth: 16,
    }
}
