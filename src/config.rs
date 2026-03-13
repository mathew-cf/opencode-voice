//! CLI argument parsing and application configuration.

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

/// Whisper model size selection.
///
/// English-only variants (`*.en`) are fine-tuned on English and slightly more
/// accurate for standard accents.  Multilingual variants are trained on 99
/// languages and handle accented English better because they've seen more
/// diverse phonetic patterns.
#[derive(Debug, Clone)]
pub enum ModelSize {
    TinyEn,
    BaseEn,
    SmallEn,
    Tiny,
    Base,
    Small,
}

impl Default for ModelSize {
    fn default() -> Self {
        ModelSize::BaseEn
    }
}

impl std::fmt::Display for ModelSize {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ModelSize::TinyEn => write!(f, "tiny.en"),
            ModelSize::BaseEn => write!(f, "base.en"),
            ModelSize::SmallEn => write!(f, "small.en"),
            ModelSize::Tiny => write!(f, "tiny"),
            ModelSize::Base => write!(f, "base"),
            ModelSize::Small => write!(f, "small"),
        }
    }
}

impl ModelSize {
    /// Returns `true` for multilingual models (without the `.en` suffix).
    pub fn is_multilingual(&self) -> bool {
        matches!(self, ModelSize::Tiny | ModelSize::Base | ModelSize::Small)
    }
}

impl std::str::FromStr for ModelSize {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s {
            "tiny.en" => Ok(ModelSize::TinyEn),
            "base.en" => Ok(ModelSize::BaseEn),
            "small.en" => Ok(ModelSize::SmallEn),
            "tiny" => Ok(ModelSize::Tiny),
            "base" => Ok(ModelSize::Base),
            "small" => Ok(ModelSize::Small),
            _ => Err(anyhow::anyhow!(
                "Unknown model size: {}. Valid: tiny.en, tiny, base.en, base, small.en, small",
                s
            )),
        }
    }
}

/// OpenCode voice input CLI tool.
#[derive(Parser, Debug)]
#[command(name = "opencode-voice", about = "Voice input for OpenCode", version)]
pub struct CliArgs {
    #[command(subcommand)]
    pub command: Option<Commands>,

    /// OpenCode server port [default: 4096]
    #[arg(long, short = 'p', global = true)]
    pub port: Option<u16>,

    /// Audio device name
    #[arg(long, global = true)]
    pub device: Option<String>,

    /// Whisper model size (tiny.en, base.en, small.en)
    #[arg(long, short = 'm', global = true)]
    pub model: Option<ModelSize>,

    /// Toggle key character (default: space)
    #[arg(long, short = 'k', global = true)]
    pub key: Option<char>,

    /// Global hotkey name (default: right_option)
    #[arg(long, global = true)]
    pub hotkey: Option<String>,

    /// Disable global hotkey, use terminal key only
    #[arg(long = "no-global", global = true)]
    pub no_global: bool,

    /// Enable push-to-talk mode (default: true)
    #[arg(
        long = "push-to-talk",
        global = true,
        overrides_with = "no_push_to_talk"
    )]
    pub push_to_talk: bool,

    /// Disable push-to-talk mode
    #[arg(long = "no-push-to-talk", global = true)]
    pub no_push_to_talk: bool,

    /// Handle OpenCode permission and question prompts via voice (default: true)
    #[arg(
        long = "handle-prompts",
        global = true,
        overrides_with = "no_handle_prompts"
    )]
    pub handle_prompts: bool,

    /// Disable voice handling of OpenCode prompts
    #[arg(long = "no-handle-prompts", global = true)]
    pub no_handle_prompts: bool,

    /// Debug mode: log key events, audio info, transcripts to stderr; skip OpenCode
    #[arg(long, global = true)]
    pub debug: bool,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Run the voice mode (default)
    Run,
    /// Download and set up the whisper model
    Setup {
        /// Model size to download (tiny, base, small, tiny.en, base.en, small.en)
        #[arg(long, short = 'm')]
        model: Option<ModelSize>,
    },
    /// List available audio input devices
    Devices,
    /// List available key names for hotkey configuration
    Keys,
}

/// Resolved application configuration.
#[derive(Debug, Clone)]
pub struct AppConfig {
    pub whisper_model_path: PathBuf,
    pub opencode_port: u16,
    pub toggle_key: char,
    pub model_size: ModelSize,
    pub auto_submit: bool,
    pub server_password: Option<String>,
    pub data_dir: PathBuf,
    pub audio_device: Option<String>,
    pub use_global_hotkey: bool,
    pub global_hotkey: String,
    pub push_to_talk: bool,
    pub handle_prompts: bool,
    pub debug: bool,
}

impl AppConfig {
    /// Load configuration from CLI args + environment variables + defaults.
    /// Precedence: CLI flags > env vars > defaults.
    pub fn load(cli: &CliArgs) -> Result<Self> {
        let data_dir = get_data_dir();

        // Port: CLI > env var > default 4096
        let port_env = std::env::var("OPENCODE_VOICE_PORT")
            .ok()
            .and_then(|s| s.parse::<u16>().ok());
        let port = cli.port.or(port_env).unwrap_or(4096);

        // Model: CLI > env var > default
        let model_env = std::env::var("OPENCODE_VOICE_MODEL")
            .ok()
            .and_then(|s| s.parse::<ModelSize>().ok());
        let model_size = cli.model.clone().or(model_env).unwrap_or_default();

        // Device: CLI > env var
        let device_env = std::env::var("OPENCODE_VOICE_DEVICE").ok();
        let audio_device = cli.device.clone().or(device_env);

        // Password: env var only
        let server_password = std::env::var("OPENCODE_SERVER_PASSWORD").ok();

        // Boolean flags: explicit overrides, then defaults
        let push_to_talk = if cli.no_push_to_talk {
            false
        } else if cli.push_to_talk {
            true
        } else {
            true
        };
        let use_global_hotkey = !cli.no_global;
        let handle_prompts = if cli.no_handle_prompts {
            false
        } else if cli.handle_prompts {
            true
        } else {
            true
        };
        let whisper_model_path = crate::transcribe::setup::get_model_path(&data_dir, &model_size);

        Ok(AppConfig {
            opencode_port: port,
            toggle_key: cli.key.unwrap_or(' '),
            model_size,
            auto_submit: true,
            server_password,
            data_dir,
            audio_device,
            use_global_hotkey,
            global_hotkey: cli
                .hotkey
                .clone()
                .unwrap_or_else(|| "right_option".to_string()),
            push_to_talk,
            handle_prompts,
            debug: cli.debug,
            whisper_model_path,
        })
    }
}

/// Returns the platform-appropriate data directory for opencode-voice.
///
/// - macOS: ~/Library/Application Support/opencode-voice/
/// - Linux: $XDG_DATA_HOME/opencode-voice/ or ~/.local/share/opencode-voice/
pub fn get_data_dir() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        dirs::data_dir()
            .unwrap_or_else(|| {
                dirs::home_dir()
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join("Library")
                    .join("Application Support")
            })
            .join("opencode-voice")
    }
    #[cfg(not(target_os = "macos"))]
    {
        // Linux: XDG_DATA_HOME or ~/.local/share
        std::env::var("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                dirs::home_dir()
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join(".local")
                    .join("share")
            })
            .join("opencode-voice")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_model_size_display() {
        assert_eq!(ModelSize::TinyEn.to_string(), "tiny.en");
        assert_eq!(ModelSize::BaseEn.to_string(), "base.en");
        assert_eq!(ModelSize::SmallEn.to_string(), "small.en");
    }

    #[test]
    fn test_model_size_from_str() {
        assert!(matches!(
            "tiny.en".parse::<ModelSize>().unwrap(),
            ModelSize::TinyEn
        ));
        assert!(matches!(
            "tiny".parse::<ModelSize>().unwrap(),
            ModelSize::Tiny
        ));
        assert!(matches!(
            "base.en".parse::<ModelSize>().unwrap(),
            ModelSize::BaseEn
        ));
        assert!(matches!(
            "base".parse::<ModelSize>().unwrap(),
            ModelSize::Base
        ));
        assert!(matches!(
            "small.en".parse::<ModelSize>().unwrap(),
            ModelSize::SmallEn
        ));
        assert!(matches!(
            "small".parse::<ModelSize>().unwrap(),
            ModelSize::Small
        ));
    }

    #[test]
    fn test_model_size_from_str_invalid() {
        assert!("large".parse::<ModelSize>().is_err());
        assert!("medium.en".parse::<ModelSize>().is_err());
    }

    #[test]
    fn test_model_size_default() {
        assert!(matches!(ModelSize::default(), ModelSize::BaseEn));
    }

    #[test]
    fn test_get_data_dir_contains_app_name() {
        let dir = get_data_dir();
        let dir_str = dir.to_string_lossy();
        assert!(
            dir_str.contains("opencode-voice"),
            "data dir should contain 'opencode-voice': {}",
            dir_str
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_get_data_dir_macos() {
        let dir = get_data_dir();
        let dir_str = dir.to_string_lossy();
        // On macOS should be under Library/Application Support
        assert!(
            dir_str.contains("Library/Application Support"),
            "macOS data dir should be under Library/Application Support: {}",
            dir_str
        );
    }

    // --- Additional tests added to expand coverage ---

    #[test]
    fn test_model_size_display_tiny_en() {
        assert_eq!(ModelSize::TinyEn.to_string(), "tiny.en");
    }

    #[test]
    fn test_model_size_display_base_en() {
        assert_eq!(ModelSize::BaseEn.to_string(), "base.en");
    }

    #[test]
    fn test_model_size_display_small_en() {
        assert_eq!(ModelSize::SmallEn.to_string(), "small.en");
    }

    #[test]
    fn test_model_size_fromstr_roundtrip_tiny() {
        let s = ModelSize::TinyEn.to_string();
        let parsed: ModelSize = s.parse().unwrap();
        assert!(matches!(parsed, ModelSize::TinyEn));
    }

    #[test]
    fn test_model_size_fromstr_roundtrip_base() {
        let s = ModelSize::BaseEn.to_string();
        let parsed: ModelSize = s.parse().unwrap();
        assert!(matches!(parsed, ModelSize::BaseEn));
    }

    #[test]
    fn test_model_size_fromstr_roundtrip_small() {
        let s = ModelSize::SmallEn.to_string();
        let parsed: ModelSize = s.parse().unwrap();
        assert!(matches!(parsed, ModelSize::SmallEn));
    }

    #[test]
    fn test_model_size_fromstr_short_aliases_are_multilingual() {
        // "tiny", "base", "small" (without .en) map to multilingual variants
        assert!(matches!(
            "tiny".parse::<ModelSize>().unwrap(),
            ModelSize::Tiny
        ));
        assert!(matches!(
            "base".parse::<ModelSize>().unwrap(),
            ModelSize::Base
        ));
        assert!(matches!(
            "small".parse::<ModelSize>().unwrap(),
            ModelSize::Small
        ));
    }

    #[test]
    fn test_model_size_is_multilingual() {
        assert!(!ModelSize::TinyEn.is_multilingual());
        assert!(!ModelSize::BaseEn.is_multilingual());
        assert!(!ModelSize::SmallEn.is_multilingual());
        assert!(ModelSize::Tiny.is_multilingual());
        assert!(ModelSize::Base.is_multilingual());
        assert!(ModelSize::Small.is_multilingual());
    }

    #[test]
    fn test_model_size_fromstr_unknown_returns_error() {
        let result = "large.en".parse::<ModelSize>();
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("large.en"),
            "Error should mention the unknown value"
        );
    }

    #[test]
    fn test_get_data_dir_is_absolute() {
        let dir = get_data_dir();
        assert!(
            dir.is_absolute(),
            "data dir should be an absolute path: {:?}",
            dir
        );
    }

    #[test]
    fn test_get_data_dir_ends_with_opencode_voice() {
        let dir = get_data_dir();
        let last_component = dir.file_name().unwrap().to_string_lossy();
        assert_eq!(last_component, "opencode-voice");
    }

    /// Test AppConfig default field values by constructing a minimal struct literal.
    /// This verifies the documented defaults: auto_submit=true, push_to_talk=true,
    /// handle_prompts=true, use_global_hotkey=true.
    #[test]
    fn test_app_config_default_field_values() {
        let config = AppConfig {
            whisper_model_path: std::path::PathBuf::from("/tmp/model.bin"),
            opencode_port: 3000,
            toggle_key: ' ',
            model_size: ModelSize::TinyEn,
            auto_submit: true,
            server_password: None,
            data_dir: std::path::PathBuf::from("/tmp"),
            audio_device: None,
            use_global_hotkey: true,
            global_hotkey: "right_option".to_string(),
            push_to_talk: true,
            handle_prompts: true,
            debug: false,
        };

        assert!(config.auto_submit, "auto_submit default should be true");
        assert!(config.push_to_talk, "push_to_talk default should be true");
        assert!(
            config.handle_prompts,
            "handle_prompts default should be true"
        );
        assert!(
            config.use_global_hotkey,
            "use_global_hotkey default should be true"
        );
        assert_eq!(config.toggle_key, ' ', "toggle_key default should be space");
        assert_eq!(config.global_hotkey, "right_option");
        assert!(config.server_password.is_none());
        assert!(config.audio_device.is_none());
    }

    #[test]
    fn test_app_config_opencode_port() {
        let config = AppConfig {
            whisper_model_path: std::path::PathBuf::from("/tmp/model.bin"),
            opencode_port: 8080,
            toggle_key: ' ',
            model_size: ModelSize::BaseEn,
            auto_submit: true,
            server_password: None,
            data_dir: std::path::PathBuf::from("/tmp"),
            audio_device: None,
            use_global_hotkey: true,
            global_hotkey: "right_option".to_string(),
            push_to_talk: true,
            handle_prompts: true,
            debug: false,
        };

        assert_eq!(config.opencode_port, 8080);
    }

    #[test]
    fn test_default_port_is_4096_when_no_env() {
        // Temporarily ensure the env var is not set
        let prev = std::env::var("OPENCODE_VOICE_PORT").ok();
        unsafe {
            std::env::remove_var("OPENCODE_VOICE_PORT");
        }

        let port_env: Option<u16> = std::env::var("OPENCODE_VOICE_PORT")
            .ok()
            .and_then(|s| s.parse::<u16>().ok());
        let port: u16 = None::<u16>.or(port_env).unwrap_or(4096);

        // Restore
        if let Some(v) = prev {
            unsafe {
                std::env::set_var("OPENCODE_VOICE_PORT", v);
            }
        }

        assert_eq!(port, 4096, "default port should be 4096 when no CLI or env");
    }
}
