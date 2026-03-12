//! macOS global hotkey backend using rdev (CGEventTap).

use anyhow::{Context, Result};
use rdev::{Event, EventType, Key};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use tokio_util::sync::CancellationToken;

use crate::state::InputEvent;

fn build_key_map() -> HashMap<&'static str, Key> {
    let mut m = HashMap::new();

    // Numpad keys
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
    m.insert("numpad_decimal", Key::Unknown(65));
    m.insert("numpad_dot", Key::Unknown(65));
    m.insert("numpad_plus", Key::KpPlus);
    m.insert("numpad_add", Key::KpPlus);
    m.insert("numpad_minus", Key::KpMinus);
    m.insert("numpad_subtract", Key::KpMinus);
    m.insert("numpad_multiply", Key::KpMultiply);
    m.insert("numpad_divide", Key::KpDivide);
    m.insert("numpad_clear", Key::Unknown(71)); // CGKeyCode 0x47
    m.insert("numpad_equals", Key::Unknown(81)); // CGKeyCode 0x51

    // Modifier keys
    m.insert("right_option", Key::AltGr);
    m.insert("right_alt", Key::AltGr);
    m.insert("left_option", Key::Alt);
    m.insert("left_alt", Key::Alt);
    m.insert("right_command", Key::MetaRight);
    m.insert("right_cmd", Key::MetaRight);
    m.insert("left_command", Key::MetaLeft);
    m.insert("left_cmd", Key::MetaLeft);
    m.insert("right_shift", Key::ShiftRight);
    m.insert("left_shift", Key::ShiftLeft);
    m.insert("right_control", Key::ControlRight);
    m.insert("right_ctrl", Key::ControlRight);
    m.insert("left_control", Key::ControlLeft);
    m.insert("left_ctrl", Key::ControlLeft);
    m.insert("fn_key", Key::Function);
    m.insert("fn", Key::Function);
    m.insert("caps_lock", Key::CapsLock);

    // Function keys F1-F12
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

    // Function keys F13-F20 (macOS virtual keycodes)
    m.insert("f13", Key::Unknown(105));
    m.insert("f14", Key::Unknown(107));
    m.insert("f15", Key::Unknown(113));
    m.insert("f16", Key::Unknown(106));
    m.insert("f17", Key::Unknown(64));
    m.insert("f18", Key::Unknown(79));
    m.insert("f19", Key::Unknown(80));
    m.insert("f20", Key::Unknown(90));

    // Common keys
    m.insert("space", Key::Space);
    m.insert("tab", Key::Tab);
    m.insert("escape", Key::Escape);
    m.insert("delete", Key::Backspace);
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
    // Section sign (§) — the ISO 102nd key (between left-shift and Z on ISO keyboards).
    m.insert("section", Key::Unknown(10)); // CGKeyCode 0x0A
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

pub(crate) fn get_key_map() -> &'static HashMap<&'static str, Key> {
    KEY_MAP.get_or_init(build_key_map)
}

/// Resolves a key name string to an rdev::Key.
fn resolve_key(input: &str) -> Result<Key> {
    let key_map = get_key_map();

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

/// Global hotkey monitor using rdev (macOS CGEventTap).
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

        // Wait briefly for immediate errors (e.g., Accessibility permission).
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
        return "Accessibility permission required for global hotkey.\n  \
                Go to: System Settings → Privacy & Security → Accessibility\n  \
                Enable your terminal app (Terminal, iTerm2, etc.)"
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
        assert_eq!(resolve_key("space").unwrap(), Key::Space);
    }

    #[test]
    fn test_resolve_key_f1() {
        assert_eq!(resolve_key("f1").unwrap(), Key::F1);
    }

    #[test]
    fn test_resolve_key_left_command() {
        assert_eq!(resolve_key("left_command").unwrap(), Key::MetaLeft);
    }

    #[test]
    fn test_resolve_key_caps_lock() {
        assert_eq!(resolve_key("caps_lock").unwrap(), Key::CapsLock);
    }

    #[test]
    fn test_resolve_key_escape() {
        assert_eq!(resolve_key("escape").unwrap(), Key::Escape);
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
}
