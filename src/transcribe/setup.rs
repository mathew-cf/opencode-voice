//! Whisper model download and setup functions.
//!
//! Handles downloading GGML model files from HuggingFace and verifying their
//! integrity before use.

use anyhow::{bail, Context, Result};
use futures::StreamExt;
use indicatif::{ProgressBar, ProgressStyle};
use std::path::PathBuf;
use tokio::fs as tokio_fs;
use tokio::io::AsyncWriteExt;

use crate::config::ModelSize;
use crate::transcribe::engine::is_model_valid;

/// Base URL for downloading whisper GGML model files.
const HUGGINGFACE_BASE_URL: &str =
    "https://huggingface.co/ggerganov/whisper.cpp/resolve/main";

/// Returns the filesystem path where the whisper model file should be stored.
///
/// Models are stored as `ggml-{model_size}.bin` inside the `models/` subdirectory
/// of the application data directory.
pub fn get_model_path(data_dir: &PathBuf, model_size: &ModelSize) -> PathBuf {
    data_dir
        .join("models")
        .join(format!("ggml-{}.bin", model_size))
}

/// Returns `true` if the whisper model file exists and is valid (> 1MB).
///
/// Uses [`is_model_valid`] from the engine module to check file existence and size.
pub fn is_whisper_ready(data_dir: &PathBuf, model_size: &ModelSize) -> bool {
    let path = get_model_path(data_dir, model_size);
    is_model_valid(&path)
}

/// Downloads the whisper GGML model from HuggingFace with a progress bar.
///
/// The file is first written to a temporary path and then atomically renamed
/// to the final destination. Creates the `models/` directory if it does not
/// exist. Returns an error if the download fails or the resulting file is
/// smaller than 1MB.
pub async fn download_model(data_dir: &PathBuf, model_size: &ModelSize) -> Result<()> {
    let models_dir = data_dir.join("models");
    tokio_fs::create_dir_all(&models_dir)
        .await
        .with_context(|| format!("Failed to create models directory: {}", models_dir.display()))?;

    let model_filename = format!("ggml-{}.bin", model_size);
    let url = format!("{}/{}", HUGGINGFACE_BASE_URL, model_filename);
    let final_path = models_dir.join(&model_filename);
    let tmp_path = models_dir.join(format!("{}.tmp", model_filename));

    println!("Downloading whisper model {} from HuggingFace…", model_size);
    println!("  URL: {}", url);

    let client = reqwest::Client::new();
    let response = client
        .get(&url)
        .send()
        .await
        .with_context(|| format!("Failed to connect to {}", url))?;

    if !response.status().is_success() {
        bail!(
            "Download failed: HTTP {} for {}",
            response.status(),
            url
        );
    }

    // Use Content-Length header to set up the progress bar total.
    let total_bytes = response.content_length();

    let pb = ProgressBar::new(total_bytes.unwrap_or(0));
    pb.set_style(
        ProgressStyle::with_template(
            "{percent}% [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})",
        )
        .unwrap_or_else(|_| ProgressStyle::default_bar())
        .progress_chars("=>-"),
    );

    // Stream response body to a temporary file.
    let mut tmp_file = tokio_fs::File::create(&tmp_path)
        .await
        .with_context(|| format!("Failed to create temp file: {}", tmp_path.display()))?;

    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.with_context(|| "Error reading download stream")?;
        tmp_file
            .write_all(&chunk)
            .await
            .with_context(|| "Failed to write chunk to temp file")?;
        pb.inc(chunk.len() as u64);
    }

    tmp_file
        .flush()
        .await
        .with_context(|| "Failed to flush temp file")?;
    drop(tmp_file);

    pb.finish_with_message("Download complete");

    // Atomically rename temp file to final destination.
    tokio_fs::rename(&tmp_path, &final_path)
        .await
        .with_context(|| {
            format!(
                "Failed to rename {} to {}",
                tmp_path.display(),
                final_path.display()
            )
        })?;

    // Verify the downloaded file is valid.
    if !is_model_valid(&final_path) {
        // Clean up the bad file before returning an error.
        tokio_fs::remove_file(&final_path).await.ok();
        bail!(
            "Downloaded model file is invalid or too small (< 1MB): {}",
            final_path.display()
        );
    }

    println!("Model saved to {}", final_path.display());
    Ok(())
}

/// Ensures the whisper model is present and valid, downloading it if necessary.
///
/// If the model already exists and passes validation, this function returns
/// immediately without downloading. Otherwise it calls [`download_model`].
pub async fn setup_whisper(data_dir: &PathBuf, model_size: &ModelSize) -> Result<()> {
    if is_whisper_ready(data_dir, model_size) {
        let path = get_model_path(data_dir, model_size);
        println!(
            "Whisper model already present at {}. Skipping download.",
            path.display()
        );
        return Ok(());
    }

    download_model(data_dir, model_size).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Helper that returns a temporary directory path that does not exist on disk.
    fn nonexistent_data_dir() -> PathBuf {
        std::env::temp_dir().join(format!(
            "opencode-voice-test-{}",
            uuid::Uuid::new_v4()
        ))
    }

    #[test]
    fn test_get_model_path() {
        let data_dir = PathBuf::from("/tmp/opencode-voice");
        let path = get_model_path(&data_dir, &ModelSize::TinyEn);
        assert_eq!(path, PathBuf::from("/tmp/opencode-voice/models/ggml-tiny.en.bin"));

        let path_base = get_model_path(&data_dir, &ModelSize::BaseEn);
        assert_eq!(path_base, PathBuf::from("/tmp/opencode-voice/models/ggml-base.en.bin"));

        let path_small = get_model_path(&data_dir, &ModelSize::SmallEn);
        assert_eq!(path_small, PathBuf::from("/tmp/opencode-voice/models/ggml-small.en.bin"));
    }

    #[test]
    fn test_is_whisper_ready_missing_file() {
        let data_dir = nonexistent_data_dir();
        // Directory and file do not exist — should return false.
        assert!(!is_whisper_ready(&data_dir, &ModelSize::TinyEn));
    }

    #[test]
    fn test_is_whisper_ready_small_file() {
        // Create a real but tiny file (< 1MB) and verify it is rejected.
        let tmp_dir = std::env::temp_dir().join(format!(
            "opencode-voice-test-small-{}",
            uuid::Uuid::new_v4()
        ));
        let models_dir = tmp_dir.join("models");
        std::fs::create_dir_all(&models_dir).unwrap();

        let model_path = models_dir.join("ggml-tiny.en.bin");
        std::fs::write(&model_path, b"this is way too small").unwrap();

        assert!(!is_whisper_ready(&tmp_dir, &ModelSize::TinyEn));

        // Cleanup
        std::fs::remove_dir_all(&tmp_dir).ok();
    }

    #[test]
    fn test_is_whisper_ready_valid_file() {
        // Create a file that is exactly 1MB + 1 byte — should be accepted.
        let tmp_dir = std::env::temp_dir().join(format!(
            "opencode-voice-test-valid-{}",
            uuid::Uuid::new_v4()
        ));
        let models_dir = tmp_dir.join("models");
        std::fs::create_dir_all(&models_dir).unwrap();

        let model_path = models_dir.join("ggml-base.en.bin");
        let big_data = vec![0u8; 1_000_001];
        std::fs::write(&model_path, &big_data).unwrap();

        assert!(is_whisper_ready(&tmp_dir, &ModelSize::BaseEn));

        // Cleanup
        std::fs::remove_dir_all(&tmp_dir).ok();
    }

    #[test]
    fn test_get_model_path_contains_model_size() {
        let data_dir = PathBuf::from("/data");
        for (size, expected_fragment) in [
            (ModelSize::TinyEn, "tiny.en"),
            (ModelSize::BaseEn, "base.en"),
            (ModelSize::SmallEn, "small.en"),
        ] {
            let path = get_model_path(&data_dir, &size);
            let path_str = path.to_string_lossy();
            assert!(
                path_str.contains(expected_fragment),
                "Expected path to contain '{}', got '{}'",
                expected_fragment,
                path_str
            );
            assert!(
                path_str.ends_with(".bin"),
                "Expected path to end with '.bin', got '{}'",
                path_str
            );
        }
    }
}
