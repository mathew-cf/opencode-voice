//! Terminal keyboard input handler using crossterm raw mode.

use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    terminal,
};
use std::io;
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

use crate::state::InputEvent;

/// Returns true if stdin is a TTY (interactive terminal).
pub fn is_tty() -> bool {
    use std::io::IsTerminal;
    io::stdin().is_terminal()
}

/// Terminal keyboard input reader using crossterm raw mode.
pub struct KeyboardInput {
    toggle_key: char,
    sender: tokio::sync::mpsc::UnboundedSender<InputEvent>,
    cancel: CancellationToken,
}

impl KeyboardInput {
    pub fn new(
        toggle_key: char,
        sender: tokio::sync::mpsc::UnboundedSender<InputEvent>,
        cancel: CancellationToken,
    ) -> Self {
        KeyboardInput {
            toggle_key,
            sender,
            cancel,
        }
    }

    /// Runs the keyboard input loop. Blocks until cancel token is fired or Quit received.
    ///
    /// Enables raw mode on entry, ALWAYS disables on exit (including errors/panics via Drop).
    pub fn run(&self) -> Result<()> {
        // Enable raw mode
        terminal::enable_raw_mode()?;

        // Ensure raw mode is disabled when this function exits (via Drop guard)
        let _guard = RawModeGuard;

        let mut last_toggle = Instant::now()
            .checked_sub(Duration::from_millis(500))
            .unwrap_or(Instant::now());

        loop {
            // Check cancellation
            if self.cancel.is_cancelled() {
                break;
            }

            // Poll for events with 100ms timeout
            if !event::poll(Duration::from_millis(100))? {
                continue;
            }

            let ev = event::read()?;

            match ev {
                Event::Key(KeyEvent {
                    code, modifiers, ..
                }) => match code {
                    // Ctrl+C → Quit
                    KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => {
                        let _ = self.sender.send(InputEvent::Quit);
                        break;
                    }
                    // 'q' → Quit
                    KeyCode::Char('q') => {
                        let _ = self.sender.send(InputEvent::Quit);
                        break;
                    }
                    // Toggle key (e.g., space)
                    KeyCode::Char(c) if c == self.toggle_key => {
                        let now = Instant::now();
                        if now.duration_since(last_toggle) >= Duration::from_millis(200) {
                            last_toggle = now;
                            let _ = self.sender.send(InputEvent::Toggle);
                        }
                    }
                    _ => {}
                },
                _ => {}
            }
        }

        Ok(())
    }
}

/// RAII guard that disables raw mode on drop.
struct RawModeGuard;

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
    }
}
