//! cpal-based microphone capture: 16kHz mono i16 PCM audio recording.
//!
//! Tries the ideal config (16kHz mono) first.  When the device doesn't support
//! it — common on macOS — falls back to the device's native sample-rate and
//! channel count and resamples to 16kHz mono in the audio callback.

use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, StreamConfig};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Lists all available audio input device names.
pub fn list_devices() -> Result<Vec<String>> {
    let host = cpal::default_host();
    let devices = host
        .input_devices()
        .context("Failed to enumerate input devices")?;
    Ok(devices.filter_map(|d| d.name().ok()).collect())
}

/// Resampling state carried across audio callbacks.
struct ResampleState {
    /// native_sample_rate / 16_000
    ratio: f64,
    /// Fractional input-sample position carried between callbacks.
    phase: f64,
}

/// Records microphone audio via cpal, always producing 16kHz mono i16 output.
pub struct CpalRecorder {
    device_name: Option<String>,
    samples: Arc<Mutex<Vec<i16>>>,
    stream: Option<cpal::Stream>,
    start_time: Option<Instant>,
    energy_tx: Option<tokio::sync::mpsc::UnboundedSender<f32>>,
}

impl CpalRecorder {
    /// Creates a new recorder for the given device (or default if None).
    pub fn new(device: Option<&str>) -> Result<Self> {
        Ok(CpalRecorder {
            device_name: device.map(|s| s.to_string()),
            samples: Arc::new(Mutex::new(Vec::new())),
            stream: None,
            start_time: None,
            energy_tx: None,
        })
    }

    /// Starts recording. Returns a receiver for RMS energy updates (0.0–1.0).
    pub fn start(&mut self) -> Result<tokio::sync::mpsc::UnboundedReceiver<f32>> {
        let host = cpal::default_host();

        // Find device
        let device = if let Some(ref name) = self.device_name {
            host.input_devices()
                .context("Failed to enumerate devices")?
                .find(|d| d.name().map(|n| n == *name).unwrap_or(false))
                .with_context(|| format!("Audio device '{}' not found", name))?
        } else {
            host.default_input_device().context(
                "No default audio input device found. Please check microphone connection.",
            )?
        };

        let (energy_tx, energy_rx) = tokio::sync::mpsc::unbounded_channel::<f32>();

        let stream = self.build_stream(&device, energy_tx.clone())?;

        stream.play().context("Failed to start audio stream")?;

        self.stream = Some(stream);
        self.start_time = Some(Instant::now());
        self.energy_tx = Some(energy_tx);

        Ok(energy_rx)
    }

    /// Builds the cpal input stream.
    ///
    /// 1. Try 16kHz mono i16 (zero conversion — ideal).
    /// 2. Try 16kHz mono f32 (format conversion only).
    /// 3. Fall back to the device's native config and resample in the callback.
    fn build_stream(
        &self,
        device: &cpal::Device,
        energy_tx: tokio::sync::mpsc::UnboundedSender<f32>,
    ) -> Result<cpal::Stream> {
        let ideal_config = StreamConfig {
            channels: 1,
            sample_rate: cpal::SampleRate(16_000),
            buffer_size: cpal::BufferSize::Default,
        };

        let debug = std::env::var("RUST_LOG").is_ok();

        // --- Strategy 1: 16kHz mono i16 (ideal) ---
        if let Ok(stream) = self.build_direct_i16_stream(device, &ideal_config, energy_tx.clone()) {
            if debug {
                eprintln!("[audio] Using 16kHz mono i16 (ideal)");
            }
            return Ok(stream);
        }

        // --- Strategy 2: 16kHz mono f32 ---
        if let Ok(stream) = self.build_direct_f32_stream(device, &ideal_config, energy_tx.clone()) {
            if debug {
                eprintln!("[audio] Using 16kHz mono f32");
            }
            return Ok(stream);
        }

        // --- Strategy 3: native config + resample ---
        let default_config = device
            .default_input_config()
            .context("Failed to get any supported input config from audio device")?;

        let native_rate = default_config.sample_rate().0;
        let native_channels = default_config.channels();
        let native_format = default_config.sample_format();

        if debug {
            eprintln!(
                "[audio] Capturing at native {}Hz {}ch {:?}, resampling to 16kHz",
                native_rate, native_channels, native_format
            );
        }

        let stream_config = StreamConfig {
            channels: native_channels,
            sample_rate: cpal::SampleRate(native_rate),
            buffer_size: cpal::BufferSize::Default,
        };

        match native_format {
            SampleFormat::I16 => self.build_resampling_i16_stream(
                device,
                &stream_config,
                native_rate,
                native_channels,
                energy_tx,
            ),
            _ => self.build_resampling_f32_stream(
                device,
                &stream_config,
                native_rate,
                native_channels,
                energy_tx,
            ),
        }
        .context("Failed to build audio input stream with any supported configuration. Check microphone permissions.")
    }

    // ---------------------------------------------------------------
    //  Direct streams (16kHz mono, no resampling)
    // ---------------------------------------------------------------

    /// 16kHz mono i16 — no conversion needed.
    fn build_direct_i16_stream(
        &self,
        device: &cpal::Device,
        config: &StreamConfig,
        energy_tx: tokio::sync::mpsc::UnboundedSender<f32>,
    ) -> Result<cpal::Stream> {
        let samples_arc = Arc::clone(&self.samples);

        let stream = device
            .build_input_stream(
                config,
                move |data: &[i16], _: &cpal::InputCallbackInfo| {
                    if !data.is_empty() {
                        let sum_sq: f64 = data
                            .iter()
                            .map(|&s| {
                                let f = s as f64 / 32768.0;
                                f * f
                            })
                            .sum();
                        let rms = (sum_sq / data.len() as f64).sqrt() as f32;
                        let _ = energy_tx.send(rms.min(1.0));
                    }
                    if let Ok(mut guard) = samples_arc.try_lock() {
                        guard.extend_from_slice(data);
                    }
                },
                |err| eprintln!("Audio stream error: {}", err),
                None,
            )
            .map_err(|e| anyhow::anyhow!("i16 stream: {}", e))?;

        Ok(stream)
    }

    /// 16kHz mono f32 — format conversion only (f32 → i16).
    fn build_direct_f32_stream(
        &self,
        device: &cpal::Device,
        config: &StreamConfig,
        energy_tx: tokio::sync::mpsc::UnboundedSender<f32>,
    ) -> Result<cpal::Stream> {
        let samples_arc = Arc::clone(&self.samples);

        let stream = device
            .build_input_stream(
                config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    if !data.is_empty() {
                        let sum_sq: f64 = data.iter().map(|&s| (s as f64) * (s as f64)).sum();
                        let rms = (sum_sq / data.len() as f64).sqrt() as f32;
                        let _ = energy_tx.send(rms.min(1.0));
                    }
                    if let Ok(mut guard) = samples_arc.try_lock() {
                        for &s in data {
                            let clamped = s.clamp(-1.0, 1.0);
                            guard.push((clamped * 32767.0) as i16);
                        }
                    }
                },
                |err| eprintln!("Audio stream error: {}", err),
                None,
            )
            .map_err(|e| anyhow::anyhow!("f32 stream: {}", e))?;

        Ok(stream)
    }

    // ---------------------------------------------------------------
    //  Resampling streams (native rate/channels → 16kHz mono)
    // ---------------------------------------------------------------

    /// Native-rate f32 stream with downmix + resample to 16kHz mono i16.
    fn build_resampling_f32_stream(
        &self,
        device: &cpal::Device,
        config: &StreamConfig,
        native_rate: u32,
        native_channels: u16,
        energy_tx: tokio::sync::mpsc::UnboundedSender<f32>,
    ) -> Result<cpal::Stream> {
        let samples_arc = Arc::clone(&self.samples);
        let state = Arc::new(Mutex::new(ResampleState {
            ratio: native_rate as f64 / 16_000.0,
            phase: 0.0,
        }));

        let stream = device
            .build_input_stream(
                config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    let ch = native_channels as usize;

                    // --- Downmix to mono ---
                    let mono: Vec<f32> = if ch > 1 {
                        data.chunks(ch)
                            .map(|frame| frame.iter().sum::<f32>() / ch as f32)
                            .collect()
                    } else {
                        data.to_vec()
                    };

                    // --- RMS energy ---
                    if !mono.is_empty() {
                        let sum_sq: f64 = mono.iter().map(|&s| (s as f64) * (s as f64)).sum();
                        let rms = (sum_sq / mono.len() as f64).sqrt() as f32;
                        let _ = energy_tx.send(rms.min(1.0));
                    }

                    // --- Resample (linear interpolation) ---
                    if let Ok(mut st) = state.lock() {
                        let ratio = st.ratio;
                        let mut phase = st.phase;
                        let len = mono.len() as f64;
                        let mut resampled = Vec::new();

                        while phase < len {
                            let idx = phase as usize;
                            let frac = (phase - idx as f64) as f32;
                            let a = mono[idx];
                            let b = if idx + 1 < mono.len() {
                                mono[idx + 1]
                            } else {
                                a
                            };
                            let sample = a + (b - a) * frac;
                            let clamped = sample.clamp(-1.0, 1.0);
                            resampled.push((clamped * 32767.0) as i16);
                            phase += ratio;
                        }

                        st.phase = phase - len;

                        if let Ok(mut guard) = samples_arc.try_lock() {
                            guard.extend_from_slice(&resampled);
                        }
                    }
                },
                |err| eprintln!("Audio stream error: {}", err),
                None,
            )
            .map_err(|e| anyhow::anyhow!("Resampling f32 stream: {}", e))?;

        Ok(stream)
    }

    /// Native-rate i16 stream with downmix + resample to 16kHz mono i16.
    fn build_resampling_i16_stream(
        &self,
        device: &cpal::Device,
        config: &StreamConfig,
        native_rate: u32,
        native_channels: u16,
        energy_tx: tokio::sync::mpsc::UnboundedSender<f32>,
    ) -> Result<cpal::Stream> {
        let samples_arc = Arc::clone(&self.samples);
        let state = Arc::new(Mutex::new(ResampleState {
            ratio: native_rate as f64 / 16_000.0,
            phase: 0.0,
        }));

        let stream = device
            .build_input_stream(
                config,
                move |data: &[i16], _: &cpal::InputCallbackInfo| {
                    let ch = native_channels as usize;

                    // --- Convert to f32 and downmix to mono ---
                    let mono: Vec<f32> = if ch > 1 {
                        data.chunks(ch)
                            .map(|frame| {
                                let sum: f32 = frame.iter().map(|&s| s as f32 / 32768.0).sum();
                                sum / ch as f32
                            })
                            .collect()
                    } else {
                        data.iter().map(|&s| s as f32 / 32768.0).collect()
                    };

                    // --- RMS energy ---
                    if !mono.is_empty() {
                        let sum_sq: f64 = mono.iter().map(|&s| (s as f64) * (s as f64)).sum();
                        let rms = (sum_sq / mono.len() as f64).sqrt() as f32;
                        let _ = energy_tx.send(rms.min(1.0));
                    }

                    // --- Resample (linear interpolation) ---
                    if let Ok(mut st) = state.lock() {
                        let ratio = st.ratio;
                        let mut phase = st.phase;
                        let len = mono.len() as f64;
                        let mut resampled = Vec::new();

                        while phase < len {
                            let idx = phase as usize;
                            let frac = (phase - idx as f64) as f32;
                            let a = mono[idx];
                            let b = if idx + 1 < mono.len() {
                                mono[idx + 1]
                            } else {
                                a
                            };
                            let sample = a + (b - a) * frac;
                            let clamped = sample.clamp(-1.0, 1.0);
                            resampled.push((clamped * 32767.0) as i16);
                            phase += ratio;
                        }

                        st.phase = phase - len;

                        if let Ok(mut guard) = samples_arc.try_lock() {
                            guard.extend_from_slice(&resampled);
                        }
                    }
                },
                |err| eprintln!("Audio stream error: {}", err),
                None,
            )
            .map_err(|e| anyhow::anyhow!("Resampling i16 stream: {}", e))?;

        Ok(stream)
    }

    /// Stops recording and returns all captured samples (16kHz mono i16).
    pub fn stop(&mut self) -> Result<Vec<i16>> {
        // Drop the stream to stop recording
        self.stream = None;
        self.energy_tx = None;

        let samples = {
            let guard = self
                .samples
                .lock()
                .map_err(|_| anyhow::anyhow!("Failed to lock samples buffer"))?;
            guard.clone()
        };

        // Clear for next use
        if let Ok(mut guard) = self.samples.lock() {
            guard.clear();
        }

        Ok(samples)
    }

    /// Returns the elapsed recording duration in seconds.
    pub fn duration(&self) -> f64 {
        self.start_time
            .map(|t| t.elapsed().as_secs_f64())
            .unwrap_or(0.0)
    }
}

// Safety: CpalRecorder is Send because Arc<Mutex<>> handles shared state.
// cpal::Stream is not Send on all platforms (e.g. macOS CoreAudio), but we
// manage it carefully: the stream is only dropped (in stop()), never accessed
// from another thread after creation.
unsafe impl Send for CpalRecorder {}
