//! Global hotkey monitoring via rdev (no Accessibility permission dance required for listen-only).

use anyhow::{Context, Result};
use rdev::{Event, EventType, Key};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use tokio_util::sync::CancellationToken;

use crate::state::InputEvent;

/// Returns the KEY_MAP mapping key names to rdev::Key variants.
fn build_key_map() -> HashMap<&'static str, Key> {
    let mut m = HashMap::new();

    // Numpad keys — rdev uses Kp0-Kp9 for numpad digits
    m.insert("numpad_0", Key::Kp0);
    m.insert("numpad_1", Key::Kp1);
    m.insert("numpad_2", Key::Kp2);
    m.insert("numpad_3", Key::Kp3);
    m.insert("numpad_4", Key::Kp4);
    m.insert("numpad_5", Key::Kp5);
    m.insert("numpad_6", Key::Kp6);
    m.insert("numpad_7", Key::Kp7);
    m.insert("numpad_8", Key::Kp8);
    m.insert("numpad_9", Key::Kp9);
    m.insert("numpad_enter", Key::KpReturn);
    // macOS keycode 0x41 = 65 for numpad decimal/dot
    m.insert("numpad_decimal", Key::Unknown(65));
    m.insert("numpad_dot", Key::Unknown(65));
    // macOS keycode 0x45 = 69 for numpad plus/add
    m.insert("numpad_plus", Key::KpPlus);
    m.insert("numpad_add", Key::KpPlus);
    m.insert("numpad_minus", Key::KpMinus);
    m.insert("numpad_subtract", Key::KpMinus);
    m.insert("numpad_multiply", Key::KpMultiply);
    m.insert("numpad_divide", Key::KpDivide);
    // macOS keycode 0x47 = 71 for numpad clear
    m.insert("numpad_clear", Key::Unknown(71));
    // macOS keycode 0x51 = 81 for numpad equals
    m.insert("numpad_equals", Key::Unknown(81));

    // Modifier keys
    m.insert("right_option", Key::AltGr);
    m.insert("right_alt", Key::AltGr); // alias
    m.insert("left_option", Key::Alt);
    m.insert("left_alt", Key::Alt); // alias
    m.insert("right_command", Key::MetaRight);
    m.insert("right_cmd", Key::MetaRight); // alias
    m.insert("left_command", Key::MetaLeft);
    m.insert("left_cmd", Key::MetaLeft); // alias
    m.insert("right_shift", Key::ShiftRight);
    m.insert("left_shift", Key::ShiftLeft);
    m.insert("right_control", Key::ControlRight);
    m.insert("right_ctrl", Key::ControlRight); // alias
    m.insert("left_control", Key::ControlLeft);
    m.insert("left_ctrl", Key::ControlLeft); // alias
    m.insert("fn_key", Key::Function);
    m.insert("fn", Key::Function); // alias
    m.insert("caps_lock", Key::CapsLock);

    // Function keys F1-F12 (rdev has native variants)
    m.insert("f1", Key::F1);
    m.insert("f2", Key::F2);
    m.insert("f3", Key::F3);
    m.insert("f4", Key::F4);
    m.insert("f5", Key::F5);
    m.insert("f6", Key::F6);
    m.insert("f7", Key::F7);
    m.insert("f8", Key::F8);
    m.insert("f9", Key::F9);
    m.insert("f10", Key::F10);
    m.insert("f11", Key::F11);
    m.insert("f12", Key::F12);

    // Function keys F13-F20 — rdev has no native variants, use macOS virtual keycodes
    // (Carbon/HIToolbox keycodes, same as TypeScript KEY_MAP)
    m.insert("f13", Key::Unknown(105)); // 0x69
    m.insert("f14", Key::Unknown(107)); // 0x6b
    m.insert("f15", Key::Unknown(113)); // 0x71
    m.insert("f16", Key::Unknown(106)); // 0x6a
    m.insert("f17", Key::Unknown(64)); // 0x40
    m.insert("f18", Key::Unknown(79)); // 0x4f
    m.insert("f19", Key::Unknown(80)); // 0x50
    m.insert("f20", Key::Unknown(90)); // 0x5a

    // Common keys
    m.insert("space", Key::Space);
    m.insert("tab", Key::Tab);
    m.insert("escape", Key::Escape);
    // "delete" = backspace on macOS (the key labeled "delete" on Mac keyboards)
    m.insert("delete", Key::Backspace);
    // "forward_delete" = the forward-delete key (fn+delete on laptops, separate key on full keyboards)
    m.insert("forward_delete", Key::Delete);
    m.insert("return_key", Key::Return);
    m.insert("return", Key::Return);
    m.insert("enter", Key::Return);
    m.insert("home", Key::Home);
    m.insert("end", Key::End);
    m.insert("page_up", Key::PageUp);
    m.insert("page_down", Key::PageDown);
    m.insert("up_arrow", Key::UpArrow);
    m.insert("down_arrow", Key::DownArrow);
    m.insert("left_arrow", Key::LeftArrow);
    m.insert("right_arrow", Key::RightArrow);
    m.insert("insert", Key::Insert);
    m.insert("print_screen", Key::PrintScreen);
    m.insert("scroll_lock", Key::ScrollLock);
    m.insert("pause", Key::Pause);
    m.insert("num_lock", Key::NumLock);

    // Punctuation / symbols
    // macOS keycode 0x0a = 10 for section sign (§)
    m.insert("section", Key::Unknown(10));
    m.insert("grave", Key::BackQuote);
    m.insert("minus", Key::Minus);
    m.insert("equal", Key::Equal);
    m.insert("left_bracket", Key::LeftBracket);
    m.insert("right_bracket", Key::RightBracket);
    m.insert("backslash", Key::BackSlash);
    m.insert("semicolon", Key::SemiColon);
    m.insert("quote", Key::Quote);
    m.insert("comma", Key::Comma);
    m.insert("period", Key::Dot);
    m.insert("slash", Key::Slash);

    m
}

static KEY_MAP: OnceLock<HashMap<&'static str, Key>> = OnceLock::new();

fn get_key_map() -> &'static HashMap<&'static str, Key> {
    KEY_MAP.get_or_init(build_key_map)
}

/// Resolves a key name string to an rdev::Key.
///
/// Supports: named keys ("right_option"), decimal numbers ("65"), hex ("0x41").
pub fn resolve_key(input: &str) -> Result<Key> {
    let key_map = get_key_map();

    // Named key lookup
    if let Some(&key) = key_map.get(input) {
        return Ok(key);
    }

    // Hex number fallback
    if let Some(hex) = input
        .strip_prefix("0x")
        .or_else(|| input.strip_prefix("0X"))
    {
        if let Ok(n) = u32::from_str_radix(hex, 16) {
            return Ok(Key::Unknown(n));
        }
    }

    // Decimal number fallback
    if let Ok(n) = input.parse::<u32>() {
        return Ok(Key::Unknown(n));
    }

    Err(anyhow::anyhow!(
        "Unknown key name: '{}'. Use 'opencode-voice keys' to list valid names.",
        input
    ))
}

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

/// Returns a sorted list of all key names.
pub fn list_key_names() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = get_key_map().keys().copied().collect();
    names.sort();
    names
}

/// Global hotkey monitor using rdev.
pub struct GlobalHotkey {
    target_key: Key,
    sender: tokio::sync::mpsc::UnboundedSender<InputEvent>,
    cancel: CancellationToken,
}

impl GlobalHotkey {
    pub fn new(
        key_name: &str,
        sender: tokio::sync::mpsc::UnboundedSender<InputEvent>,
        cancel: CancellationToken,
    ) -> Result<Self> {
        let target_key =
            resolve_key(key_name).with_context(|| format!("Invalid hotkey: {}", key_name))?;
        Ok(GlobalHotkey {
            target_key,
            sender,
            cancel,
        })
    }

    /// Starts the global hotkey listener on a dedicated OS thread.
    ///
    /// rdev::listen MUST run on a non-tokio thread.
    pub fn run(&self) -> Result<()> {
        let target_key = self.target_key;
        let sender = self.sender.clone();
        let cancel = self.cancel.clone();
        let pressed = Arc::new(Mutex::new(false));

        let (result_tx, result_rx) = std::sync::mpsc::channel::<Result<()>>();
        let result_tx_clone = result_tx.clone();

        std::thread::spawn(move || {
            let result = rdev::listen(move |event: Event| {
                if cancel.is_cancelled() {
                    return;
                }

                match &event.event_type {
                    EventType::KeyPress(key) => {
                        if *key == target_key {
                            let mut p = pressed.lock().unwrap();
                            if !*p {
                                *p = true;
                                let _ = sender.send(InputEvent::KeyDown);
                            }
                        }
                    }
                    EventType::KeyRelease(key) => {
                        if *key == target_key {
                            let mut p = pressed.lock().unwrap();
                            *p = false;
                            // Send both KeyUp AND Toggle on release (matching TypeScript behavior)
                            let _ = sender.send(InputEvent::KeyUp);
                            let _ = sender.send(InputEvent::Toggle);
                        }
                    }
                    _ => {}
                }
            });

            match result {
                Ok(_) => {}
                Err(e) => {
                    let msg = format_rdev_error(&e);
                    let _ = result_tx_clone.send(Err(anyhow::anyhow!("{}", msg)));
                }
            }
        });

        // Wait briefly for immediate errors (e.g., Accessibility permission)
        std::thread::sleep(std::time::Duration::from_millis(100));
        if let Ok(Err(e)) = result_rx.try_recv() {
            return Err(e);
        }

        Ok(())
    }
}

fn format_rdev_error(error: &rdev::ListenError) -> String {
    let msg = format!("{:?}", error);
    if msg.contains("FailedToOpenX11")
        || msg.contains("AccessDenied")
        || msg.contains("PermissionDenied")
        || msg.contains("EventTapError")
    {
        #[cfg(target_os = "macos")]
        return "Accessibility permission required for global hotkey.\n  \
                Go to: System Settings → Privacy & Security → Accessibility\n  \
                Enable your terminal app (Terminal, iTerm2, etc.)"
            .to_string();
        #[cfg(not(target_os = "macos"))]
        return "Input monitoring permission required.\n  \
                Add your user to the 'input' group: sudo usermod -a -G input $USER\n  \
                Or run with appropriate permissions."
            .to_string();
    }
    format!("Global hotkey error: {}", msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_key_right_option() {
        let result = resolve_key("right_option");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Key::AltGr);
    }

    #[test]
    fn test_resolve_key_alias_right_alt() {
        // right_alt is an alias for right_option
        let k1 = resolve_key("right_option").unwrap();
        let k2 = resolve_key("right_alt").unwrap();
        assert_eq!(format!("{:?}", k1), format!("{:?}", k2));
    }

    #[test]
    fn test_resolve_key_decimal_number() {
        let result = resolve_key("65");
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), Key::Unknown(65)));
    }

    #[test]
    fn test_resolve_key_hex_number() {
        let result = resolve_key("0x41");
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), Key::Unknown(65)));
    }

    #[test]
    fn test_resolve_key_unknown() {
        let result = resolve_key("not_a_key");
        assert!(result.is_err());
    }

    #[test]
    fn test_resolve_key_space() {
        let result = resolve_key("space");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Key::Space);
    }

    #[test]
    fn test_resolve_key_f1() {
        let result = resolve_key("f1");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Key::F1);
    }

    #[test]
    fn test_resolve_key_f13() {
        // F13 uses Unknown with macOS keycode 0x69 = 105
        let result = resolve_key("f13");
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), Key::Unknown(105)));
    }

    #[test]
    fn test_resolve_key_numpad_0() {
        let result = resolve_key("numpad_0");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Key::Kp0);
    }

    #[test]
    fn test_resolve_key_numpad_enter() {
        let result = resolve_key("numpad_enter");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Key::KpReturn);
    }

    #[test]
    fn test_resolve_key_hex_uppercase() {
        let result = resolve_key("0X41");
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), Key::Unknown(65)));
    }

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
        let map = get_key_map();
        assert!(
            map.len() >= 60,
            "KEY_MAP should have at least 60 entries, has {}",
            map.len()
        );
    }

    #[test]
    fn test_resolve_key_left_command() {
        let result = resolve_key("left_command");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Key::MetaLeft);
    }

    #[test]
    fn test_resolve_key_caps_lock() {
        let result = resolve_key("caps_lock");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Key::CapsLock);
    }

    #[test]
    fn test_resolve_key_escape() {
        let result = resolve_key("escape");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Key::Escape);
    }
}
