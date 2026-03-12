mod config;
mod state;
mod app;
mod audio;
mod bridge;
mod input;
mod transcribe;
mod approval;
mod ui;

use anyhow::Result;
use clap::Parser;

use crate::config::{CliArgs, Commands, ModelSize, get_data_dir};
use crate::transcribe::setup::{is_whisper_ready, setup_whisper};

/// Reads a line of input from stdin asynchronously, printing a prompt first.
///
/// Uses `tokio::io::AsyncBufReadExt` to avoid blocking the async runtime.
/// Returns the trimmed line, or an error if stdin is closed or an I/O error occurs.
async fn read_line(prompt: &str) -> Result<String> {
    use tokio::io::{AsyncBufReadExt, BufReader};

    print!("{}", prompt);
    // Flush stdout so the prompt appears before we wait for input.
    use std::io::Write;
    std::io::stdout().flush()?;

    let stdin = tokio::io::stdin();
    let mut reader = BufReader::new(stdin);
    let mut line = String::new();
    reader.read_line(&mut line).await?;
    Ok(line.trim().to_string())
}

/// Interactively prompts the user to choose a Whisper model size.
///
/// Presents a numbered menu (1=tiny.en, 2=base.en, 3=small.en) and loops
/// until a valid choice is entered.  Returns the selected [`ModelSize`].
async fn prompt_model_choice() -> Result<ModelSize> {
    println!("Choose a Whisper model size:");
    println!("  1) tiny.en  — fastest, least accurate (~75 MB)");
    println!("  2) base.en  — balanced speed and accuracy (~142 MB)");
    println!("  3) small.en — most accurate, slower (~466 MB)");

    loop {
        let choice = read_line("Enter choice [1-3] (default: 2): ").await?;
        let model = match choice.as_str() {
            "1" => ModelSize::TinyEn,
            "" | "2" => ModelSize::BaseEn,
            "3" => ModelSize::SmallEn,
            other => {
                eprintln!("Invalid choice '{}'. Please enter 1, 2, or 3.", other);
                continue;
            }
        };
        return Ok(model);
    }
}

/// Entry point for the `opencode-voice` CLI.
///
/// Parses CLI arguments via clap, dispatches to the appropriate subcommand
/// handler, or starts the main [`app::VoiceApp`] event loop when no subcommand
/// is given.  All fatal errors are printed to stderr and the process exits with
/// code 1.
#[tokio::main]
async fn main() {
    // Suppress noisy ALSA/JACK/PulseAudio warnings from cpal's C libraries.
    // These write directly to stderr and cannot be caught by Rust.
    if std::env::var_os("PIPEWIRE_LOG_LEVEL").is_none() {
        std::env::set_var("PIPEWIRE_LOG_LEVEL", "0");
    }
    if std::env::var_os("JACK_NO_START_SERVER").is_none() {
        std::env::set_var("JACK_NO_START_SERVER", "1");
    }
    if std::env::var_os("JACK_NO_AUDIO_RESERVATION").is_none() {
        std::env::set_var("JACK_NO_AUDIO_RESERVATION", "1");
    }

    if let Err(e) = run().await {
        eprintln!("Error: {:#}", e);
        std::process::exit(1);
    }
}

/// Inner async entry point — returns `Result` so errors propagate cleanly to
/// `main`, which converts them to a non-zero exit code.
async fn run() -> Result<()> {
    let cli = CliArgs::parse();

    match &cli.command {
        // ── setup ──────────────────────────────────────────────────────────
        Some(Commands::Setup { model }) => {
            let data_dir = get_data_dir();

            // If the user didn't specify a model on the command line, ask them.
            let model_size = match model {
                Some(m) => m.clone(),
                None => prompt_model_choice().await?,
            };

            println!("Setting up Whisper model: {}", model_size);
            setup_whisper(&data_dir, &model_size).await?;
            println!("Setup complete.");
        }

        // ── devices ────────────────────────────────────────────────────────
        Some(Commands::Devices) => {
            let devices = crate::audio::capture::list_devices()?;
            if devices.is_empty() {
                println!("No audio input devices found.");
            } else {
                for device in devices {
                    println!("{}", device);
                }
            }
        }

        // ── keys ───────────────────────────────────────────────────────────
        Some(Commands::Keys) => {
            let names = crate::input::hotkey::list_key_names();
            for name in names {
                let display = crate::input::hotkey::format_key_name(name);
                println!("{:<20} {}", name, display);
            }
        }

        // ── run (explicit) or no subcommand ────────────────────────────────
        Some(Commands::Run) | None => {
            // Load configuration — this validates required flags (e.g. --port).
            let config = crate::config::AppConfig::load(&cli)?;

            // Check whether the Whisper model is ready.
            if !is_whisper_ready(&config.data_dir, &config.model_size) {
                eprintln!(
                    "Whisper model '{}' is not downloaded yet.",
                    config.model_size
                );
                eprintln!(
                    "The model is required for speech-to-text transcription."
                );

                let answer = read_line("Download it now? [y/N]: ").await?;
                if answer.eq_ignore_ascii_case("y") || answer.eq_ignore_ascii_case("yes") {
                    setup_whisper(&config.data_dir, &config.model_size).await?;
                } else {
                    eprintln!(
                        "Model download skipped. Run 'opencode-voice setup' to download it later."
                    );
                    std::process::exit(1);
                }
            }

            // Create and start the voice application.
            let mut app = crate::app::VoiceApp::new(config)?;
            app.start().await?;

            // Force-exit the process. The global hotkey listener thread
            // blocks forever on input events and cannot be cancelled, so a
            // graceful join is not possible.  All cleanup (display clear,
            // cancellation token, etc.) has already been performed by
            // VoiceApp::shutdown() before we reach this point.
            std::process::exit(0);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify that `format_key_name` produces the expected display string for
    /// a few representative key names — exercised here to confirm the import
    /// path used in `main.rs` resolves correctly.
    #[test]
    fn test_format_key_name_via_main() {
        assert_eq!(
            crate::input::hotkey::format_key_name("right_option"),
            "Right Option"
        );
        assert_eq!(
            crate::input::hotkey::format_key_name("space"),
            "Space"
        );
        assert_eq!(
            crate::input::hotkey::format_key_name("f1"),
            "F1"
        );
    }

    /// Verify that `list_key_names` returns a non-empty sorted list.
    #[test]
    fn test_list_key_names_non_empty() {
        let names = crate::input::hotkey::list_key_names();
        assert!(!names.is_empty());
        // Sorted
        assert!(names.windows(2).all(|w| w[0] <= w[1]));
    }

    /// Verify that `list_devices` does not panic (it may return an empty list
    /// in CI environments without audio hardware).
    #[test]
    fn test_list_devices_does_not_panic() {
        // We don't assert on the contents — just that it doesn't panic.
        let _ = crate::audio::capture::list_devices();
    }

    /// Verify that `get_data_dir` returns a path containing "opencode-voice".
    #[test]
    fn test_get_data_dir_contains_app_name() {
        let dir = get_data_dir();
        assert!(
            dir.to_string_lossy().contains("opencode-voice"),
            "data dir should contain 'opencode-voice': {}",
            dir.display()
        );
    }

    /// Verify that `ModelSize` variants display correctly — used in the setup
    /// prompt output.
    #[test]
    fn test_model_size_display_in_main() {
        assert_eq!(ModelSize::TinyEn.to_string(), "tiny.en");
        assert_eq!(ModelSize::BaseEn.to_string(), "base.en");
        assert_eq!(ModelSize::SmallEn.to_string(), "small.en");
    }
}
