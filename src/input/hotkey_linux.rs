//! Linux global hotkey backend using evdev (kernel input subsystem).
//!
//! Reads key events directly from `/dev/input/event*` devices, bypassing
//! the display server entirely. Works on X11, Wayland, and headless.
//!
//! Requires the user to be in the `input` group (or root).

use anyhow::{Context, Result};
use evdev::{Device, InputEventKind, Key};
use std::collections::HashMap;
use std::sync::OnceLock;
use tokio_util::sync::CancellationToken;

use crate::state::InputEvent;

fn build_key_map() -> HashMap<&'static str, Key> {
    let mut m = HashMap::new();

    // Numpad keys
    m.insert("numpad_0", Key::KEY_KP0);
    m.insert("numpad_1", Key::KEY_KP1);
    m.insert("numpad_2", Key::KEY_KP2);
    m.insert("numpad_3", Key::KEY_KP3);
    m.insert("numpad_4", Key::KEY_KP4);
    m.insert("numpad_5", Key::KEY_KP5);
    m.insert("numpad_6", Key::KEY_KP6);
    m.insert("numpad_7", Key::KEY_KP7);
    m.insert("numpad_8", Key::KEY_KP8);
    m.insert("numpad_9", Key::KEY_KP9);
    m.insert("numpad_enter", Key::KEY_KPENTER);
    m.insert("numpad_decimal", Key::KEY_KPDOT);
    m.insert("numpad_dot", Key::KEY_KPDOT);
    m.insert("numpad_plus", Key::KEY_KPPLUS);
    m.insert("numpad_add", Key::KEY_KPPLUS);
    m.insert("numpad_minus", Key::KEY_KPMINUS);
    m.insert("numpad_subtract", Key::KEY_KPMINUS);
    m.insert("numpad_multiply", Key::KEY_KPASTERISK);
    m.insert("numpad_divide", Key::KEY_KPSLASH);
    m.insert("numpad_clear", Key::KEY_NUMLOCK); // macOS "Clear" is Num Lock position on PC
    m.insert("numpad_equals", Key::KEY_KPEQUAL);

    // Modifier keys
    m.insert("right_option", Key::KEY_RIGHTALT);
    m.insert("right_alt", Key::KEY_RIGHTALT);
    m.insert("left_option", Key::KEY_LEFTALT);
    m.insert("left_alt", Key::KEY_LEFTALT);
    m.insert("right_command", Key::KEY_RIGHTMETA);
    m.insert("right_cmd", Key::KEY_RIGHTMETA);
    m.insert("left_command", Key::KEY_LEFTMETA);
    m.insert("left_cmd", Key::KEY_LEFTMETA);
    m.insert("right_shift", Key::KEY_RIGHTSHIFT);
    m.insert("left_shift", Key::KEY_LEFTSHIFT);
    m.insert("right_control", Key::KEY_RIGHTCTRL);
    m.insert("right_ctrl", Key::KEY_RIGHTCTRL);
    m.insert("left_control", Key::KEY_LEFTCTRL);
    m.insert("left_ctrl", Key::KEY_LEFTCTRL);
    m.insert("fn_key", Key::KEY_FN);
    m.insert("fn", Key::KEY_FN);
    m.insert("caps_lock", Key::KEY_CAPSLOCK);

    // Function keys F1-F12
    m.insert("f1", Key::KEY_F1);
    m.insert("f2", Key::KEY_F2);
    m.insert("f3", Key::KEY_F3);
    m.insert("f4", Key::KEY_F4);
    m.insert("f5", Key::KEY_F5);
    m.insert("f6", Key::KEY_F6);
    m.insert("f7", Key::KEY_F7);
    m.insert("f8", Key::KEY_F8);
    m.insert("f9", Key::KEY_F9);
    m.insert("f10", Key::KEY_F10);
    m.insert("f11", Key::KEY_F11);
    m.insert("f12", Key::KEY_F12);

    // Function keys F13-F20 (native evdev codes, unlike macOS raw keycodes)
    m.insert("f13", Key::KEY_F13);
    m.insert("f14", Key::KEY_F14);
    m.insert("f15", Key::KEY_F15);
    m.insert("f16", Key::KEY_F16);
    m.insert("f17", Key::KEY_F17);
    m.insert("f18", Key::KEY_F18);
    m.insert("f19", Key::KEY_F19);
    m.insert("f20", Key::KEY_F20);

    // Common keys
    m.insert("space", Key::KEY_SPACE);
    m.insert("tab", Key::KEY_TAB);
    m.insert("escape", Key::KEY_ESC);
    m.insert("delete", Key::KEY_BACKSPACE);
    m.insert("forward_delete", Key::KEY_DELETE);
    m.insert("return_key", Key::KEY_ENTER);
    m.insert("return", Key::KEY_ENTER);
    m.insert("enter", Key::KEY_ENTER);
    m.insert("home", Key::KEY_HOME);
    m.insert("end", Key::KEY_END);
    m.insert("page_up", Key::KEY_PAGEUP);
    m.insert("page_down", Key::KEY_PAGEDOWN);
    m.insert("up_arrow", Key::KEY_UP);
    m.insert("down_arrow", Key::KEY_DOWN);
    m.insert("left_arrow", Key::KEY_LEFT);
    m.insert("right_arrow", Key::KEY_RIGHT);
    m.insert("insert", Key::KEY_INSERT);
    m.insert("print_screen", Key::KEY_SYSRQ);
    m.insert("scroll_lock", Key::KEY_SCROLLLOCK);
    m.insert("pause", Key::KEY_PAUSE);
    m.insert("num_lock", Key::KEY_NUMLOCK);

    // Punctuation / symbols
    // Section sign (§) — the ISO 102nd key. On evdev this is KEY_102ND.
    m.insert("section", Key::KEY_102ND);
    m.insert("grave", Key::KEY_GRAVE);
    m.insert("minus", Key::KEY_MINUS);
    m.insert("equal", Key::KEY_EQUAL);
    m.insert("left_bracket", Key::KEY_LEFTBRACE);
    m.insert("right_bracket", Key::KEY_RIGHTBRACE);
    m.insert("backslash", Key::KEY_BACKSLASH);
    m.insert("semicolon", Key::KEY_SEMICOLON);
    m.insert("quote", Key::KEY_APOSTROPHE);
    m.insert("comma", Key::KEY_COMMA);
    m.insert("period", Key::KEY_DOT);
    m.insert("slash", Key::KEY_SLASH);

    m
}

static KEY_MAP: OnceLock<HashMap<&'static str, Key>> = OnceLock::new();

pub(crate) fn get_key_map() -> &'static HashMap<&'static str, Key> {
    KEY_MAP.get_or_init(build_key_map)
}

/// Resolves a key name string to an evdev Key.
fn resolve_key(input: &str) -> Result<Key> {
    let key_map = get_key_map();

    if let Some(&key) = key_map.get(input) {
        return Ok(key);
    }

    // Hex number fallback (Linux evdev keycode)
    if let Some(hex) = input
        .strip_prefix("0x")
        .or_else(|| input.strip_prefix("0X"))
    {
        if let Ok(n) = u16::from_str_radix(hex, 16) {
            return Ok(Key::new(n));
        }
    }

    // Decimal number fallback
    if let Ok(n) = input.parse::<u16>() {
        return Ok(Key::new(n));
    }

    Err(anyhow::anyhow!(
        "Unknown key name: '{}'. Use 'opencode-voice keys' to list valid names.",
        input
    ))
}

/// Finds all input devices that report the given key.
fn find_devices_with_key(key: Key) -> Vec<Device> {
    evdev::enumerate()
        .filter_map(|(_, device)| {
            let supports_key = device
                .supported_keys()
                .map_or(false, |keys| keys.contains(key));
            if supports_key {
                Some(device)
            } else {
                None
            }
        })
        .collect()
}

/// Global hotkey monitor using evdev (Linux kernel input).
pub struct GlobalHotkey {
    target_key: Key,
    sender: tokio::sync::mpsc::UnboundedSender<InputEvent>,
    cancel: CancellationToken,
}

impl GlobalHotkey {
    /// Creates a new global hotkey monitor.
    ///
    /// Validates the key name and verifies that at least one input device
    /// supports the target key. Returns an error immediately if no suitable
    /// devices are found (rather than failing silently in a background thread).
    pub fn new(
        key_name: &str,
        sender: tokio::sync::mpsc::UnboundedSender<InputEvent>,
        cancel: CancellationToken,
    ) -> Result<Self> {
        let target_key =
            resolve_key(key_name).with_context(|| format!("Invalid hotkey: {}", key_name))?;

        // Check for suitable devices now so the caller gets a clear error.
        let probe = find_devices_with_key(target_key);
        if probe.is_empty() {
            return Err(diagnose_no_devices(target_key));
        }

        Ok(GlobalHotkey {
            target_key,
            sender,
            cancel,
        })
    }

    /// Starts the global hotkey listener.
    ///
    /// Spawns one OS thread per keyboard device that supports the target key.
    /// Each thread blocks on `fetch_events()` and forwards matching key events
    /// through the channel.
    pub fn run(&self) -> Result<()> {
        // Re-enumerate devices (they were validated in new() but may have
        // changed; this also avoids storing Device which may not be Send).
        let devices = find_devices_with_key(self.target_key);

        for device in devices {
            let target_key = self.target_key;
            let sender = self.sender.clone();
            let cancel = self.cancel.clone();

            std::thread::spawn(move || {
                listen_on_device(device, target_key, sender, cancel);
            });
        }

        Ok(())
    }
}

/// Produces a detailed error when no devices support the target key.
///
/// Distinguishes three cases:
/// 1. No input devices accessible at all → permission issue.
/// 2. Some devices accessible but no keyboards → probably can't open keyboard
///    devices (permission issue for those specific devices).
/// 3. Keyboards accessible but none support the target key → wrong key choice.
fn diagnose_no_devices(target_key: Key) -> anyhow::Error {
    let accessible: Vec<_> = evdev::enumerate().collect();

    if accessible.is_empty() {
        return anyhow::anyhow!(
            "No input devices accessible. Global hotkey requires read access to /dev/input/.\n  \
             Fix: sudo usermod -a -G input $USER  (then log out and back in)"
        );
    }

    // Check how many accessible devices look like keyboards (support KEY_SPACE).
    let keyboards: Vec<_> = accessible
        .iter()
        .filter(|(_, d)| {
            d.supported_keys()
                .map_or(false, |keys| keys.contains(Key::KEY_SPACE))
        })
        .collect();

    // Count total device nodes to detect permission gaps.
    let total_device_nodes = std::fs::read_dir("/dev/input")
        .map(|rd| {
            rd.filter(|e| {
                e.as_ref().map_or(false, |e| {
                    e.file_name().to_string_lossy().starts_with("event")
                })
            })
            .count()
        })
        .unwrap_or(0);

    if keyboards.is_empty() {
        // We can open some devices but none are keyboards.
        anyhow::anyhow!(
            "Cannot access keyboard input devices ({} of {} /dev/input/event* devices accessible, \
             but none are keyboards).\n  \
             Fix: sudo usermod -a -G input $USER  (then log out and back in)\n  \
             Or run with --no-global for terminal-only input.",
            accessible.len(),
            total_device_nodes,
        )
    } else {
        // Keyboards are accessible but don't have the target key.
        let kb_names: Vec<String> = keyboards
            .iter()
            .filter_map(|(_, d)| d.name().map(|s| s.to_string()))
            .collect();
        anyhow::anyhow!(
            "Found {} keyboard(s) ({}) but none report support for key {:?}.\n  \
             Try a different key with --hotkey, or use --no-global for terminal-only input.",
            keyboards.len(),
            kb_names.join(", "),
            target_key,
        )
    }
}

/// Reads key events from a single evdev device in a blocking loop.
fn listen_on_device(
    mut device: Device,
    target_key: Key,
    sender: tokio::sync::mpsc::UnboundedSender<InputEvent>,
    cancel: CancellationToken,
) {
    let mut pressed = false;

    loop {
        if cancel.is_cancelled() {
            break;
        }

        let events = match device.fetch_events() {
            Ok(events) => events,
            Err(_) => break, // Device error (disconnected, etc.)
        };

        for ev in events {
            if cancel.is_cancelled() {
                return;
            }

            if let InputEventKind::Key(key) = ev.kind() {
                if key == target_key {
                    match ev.value() {
                        1 => {
                            // Press
                            if !pressed {
                                pressed = true;
                                let _ = sender.send(InputEvent::KeyDown);
                            }
                        }
                        0 => {
                            // Release
                            pressed = false;
                            let _ = sender.send(InputEvent::KeyUp);
                            let _ = sender.send(InputEvent::Toggle);
                        }
                        _ => {} // Repeat (value 2) — ignored
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_key_right_option() {
        let result = resolve_key("right_option");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Key::KEY_RIGHTALT);
    }

    #[test]
    fn test_resolve_key_alias_right_alt() {
        let k1 = resolve_key("right_option").unwrap();
        let k2 = resolve_key("right_alt").unwrap();
        assert_eq!(k1, k2);
    }

    #[test]
    fn test_resolve_key_decimal_number() {
        let result = resolve_key("57"); // KEY_SPACE = 57
        assert!(result.is_ok());
    }

    #[test]
    fn test_resolve_key_hex_number() {
        let result = resolve_key("0x39"); // KEY_SPACE = 0x39 = 57
        assert!(result.is_ok());
    }

    #[test]
    fn test_resolve_key_unknown() {
        let result = resolve_key("not_a_key");
        assert!(result.is_err());
    }

    #[test]
    fn test_resolve_key_space() {
        assert_eq!(resolve_key("space").unwrap(), Key::KEY_SPACE);
    }

    #[test]
    fn test_resolve_key_f1() {
        assert_eq!(resolve_key("f1").unwrap(), Key::KEY_F1);
    }

    #[test]
    fn test_resolve_key_f13() {
        assert_eq!(resolve_key("f13").unwrap(), Key::KEY_F13);
    }

    #[test]
    fn test_resolve_key_left_command() {
        assert_eq!(resolve_key("left_command").unwrap(), Key::KEY_LEFTMETA);
    }

    #[test]
    fn test_resolve_key_caps_lock() {
        assert_eq!(resolve_key("caps_lock").unwrap(), Key::KEY_CAPSLOCK);
    }

    #[test]
    fn test_resolve_key_escape() {
        assert_eq!(resolve_key("escape").unwrap(), Key::KEY_ESC);
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
    fn test_resolve_key_modifiers() {
        assert_eq!(resolve_key("left_alt").unwrap(), Key::KEY_LEFTALT);
        assert_eq!(resolve_key("left_shift").unwrap(), Key::KEY_LEFTSHIFT);
        assert_eq!(resolve_key("left_ctrl").unwrap(), Key::KEY_LEFTCTRL);
        assert_eq!(resolve_key("right_shift").unwrap(), Key::KEY_RIGHTSHIFT);
        assert_eq!(resolve_key("right_ctrl").unwrap(), Key::KEY_RIGHTCTRL);
    }

    #[test]
    fn test_resolve_key_numpad() {
        assert_eq!(resolve_key("numpad_0").unwrap(), Key::KEY_KP0);
        assert_eq!(resolve_key("numpad_enter").unwrap(), Key::KEY_KPENTER);
        assert_eq!(resolve_key("numpad_plus").unwrap(), Key::KEY_KPPLUS);
        assert_eq!(resolve_key("numpad_add").unwrap(), Key::KEY_KPPLUS);
    }
}
