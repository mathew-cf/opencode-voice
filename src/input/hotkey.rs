//! Global hotkey support — shared key-name utilities and platform-specific backends.
//!
//! Key name lists and formatting live here. The actual hotkey listener
//! (`GlobalHotkey`) is platform-specific:
//! - macOS: `hotkey_macos.rs` (rdev / CGEventTap)
//! - Linux: `hotkey_linux.rs` (evdev / /dev/input)

#[cfg(target_os = "macos")]
#[path = "hotkey_macos.rs"]
mod platform;

#[cfg(target_os = "linux")]
#[path = "hotkey_linux.rs"]
mod platform;

// Re-export the platform-specific GlobalHotkey so callers use
// `crate::input::hotkey::GlobalHotkey` regardless of platform.
pub use platform::GlobalHotkey;

/// Formats a key name for display (e.g., "right_option" → "Right Option").
pub fn format_key_name(input: &str) -> String {
    input
        .split('_')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                None => String::new(),
                Some(c) => c.to_uppercase().to_string() + chars.as_str(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Returns a sorted list of all supported key names.
///
/// Delegates to the platform key map so the list reflects exactly which
/// keys are available on this platform.
pub fn list_key_names() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = platform::get_key_map().keys().copied().collect();
    names.sort();
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_key_name_right_option() {
        assert_eq!(format_key_name("right_option"), "Right Option");
    }

    #[test]
    fn test_format_key_name_f13() {
        assert_eq!(format_key_name("f13"), "F13");
    }

    #[test]
    fn test_format_key_name_numpad_enter() {
        assert_eq!(format_key_name("numpad_enter"), "Numpad Enter");
    }

    #[test]
    fn test_format_key_name_space() {
        assert_eq!(format_key_name("space"), "Space");
    }

    #[test]
    fn test_list_key_names_sorted() {
        let names = list_key_names();
        assert!(!names.is_empty());
        assert!(names.windows(2).all(|w| w[0] <= w[1]));
        assert!(names.contains(&"right_option"));
        assert!(names.contains(&"space"));
        assert!(names.contains(&"f1"));
    }

    #[test]
    fn test_key_map_has_60_plus_entries() {
        let names = list_key_names();
        assert!(
            names.len() >= 60,
            "KEY_MAP should have at least 60 entries, has {}",
            names.len()
        );
    }

    #[test]
    fn test_list_key_names_includes_section() {
        let names = list_key_names();
        assert!(names.contains(&"section"));
    }

    #[test]
    fn test_list_key_names_includes_numpad_clear() {
        let names = list_key_names();
        assert!(names.contains(&"numpad_clear"));
    }
}
