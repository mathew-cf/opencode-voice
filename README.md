# opencode-voice

**Voice control for OpenCode AI coding assistant**

`[Apache 2.0]` `[Rust >=1.70]`

---

## Overview

`opencode-voice` is a standalone CLI tool that captures microphone audio, transcribes it locally using [whisper-rs](https://github.com/tazz4843/whisper-rs) (bindings to whisper.cpp), and injects the resulting text into OpenCode via its HTTP API.

Everything runs on your machine — no cloud services, no API keys, no audio leaves your device.

```
Microphone → cpal → PCM → whisper-rs → text → OpenCode HTTP API → prompt
```

**Key features**

- **Local transcription** — whisper.cpp runs entirely offline, no cloud, no API keys
- **Push-to-talk** — hold a key to record, release to transcribe and inject
- **Global hotkeys** — optional system-wide hotkey support via rdev (no terminal focus required)
- **Approval mode** — voice-driven permission and question handling for OpenCode
- **Compact terminal UI** — live status display with recording level meter and last transcript preview
- **Configurable** — model size, audio device, toggle key, and more

---

## Prerequisites

### Rust toolchain (1.70+)

Install from [https://rustup.rs](https://rustup.rs):

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

### cmake (required to build whisper-rs)

```bash
# macOS
brew install cmake

# Linux (Debian/Ubuntu)
sudo apt install cmake

# Linux (Fedora)
sudo dnf install cmake
```

### C compiler

- **macOS**: Xcode Command Line Tools — `xcode-select --install`
- **Linux**: `gcc` or `clang` (usually pre-installed)

### OpenCode with HTTP server enabled

OpenCode does **not** expose an HTTP server by default. You must start it with the `--port` flag:

```bash
opencode --port 4096
```

---

## Build

```bash
git clone <repo-url>
cd opencode-voicemode
cargo build --release
```

The binary is produced at `target/release/opencode-voice`.

---

## Install

```bash
cargo install --path .
```

This installs `opencode-voice` to `~/.cargo/bin/` (which should be on your `$PATH` after installing Rust).

---

## Setup (First Run)

Download a transcription model:

```bash
opencode-voice setup
```

This downloads the GGML model file (default: `base.en`, ~150 MB) to the platform data directory:

| Platform | Path |
|----------|------|
| macOS | `~/Library/Application Support/opencode-voice/` |
| Linux | `~/.local/share/opencode-voice/` |

### Model options

| Model | Size | Speed | Accuracy |
|-------|------|-------|----------|
| `tiny.en` | ~75 MB | Fastest | Basic |
| `base.en` | ~150 MB | Balanced | Good (default) |
| `small.en` | ~500 MB | Slower | Best |

To set up with a specific model:

```bash
opencode-voice setup --model small.en
```

---

## Usage

**Step 1** — Start OpenCode with the HTTP server enabled:

```bash
opencode --port 4096
```

**Step 2** — In a separate terminal, start opencode-voice:

```bash
opencode-voice --port 4096
```

### Push-to-talk (default)

Hold the toggle key to record, release to transcribe and send:

| Key | Action |
|-----|--------|
| `[space]` (hold) | Start recording |
| `[space]` (release) | Stop recording and transcribe |
| `q` or `Ctrl+C` | Quit |

### Toggle mode

With `--no-push-to-talk`, press to start recording, press again to stop:

| Key | Action |
|-----|--------|
| `[space]` | Start recording |
| `[space]` | Stop recording and transcribe |
| `q` or `Ctrl+C` | Quit |

---

## Subcommands

```
opencode-voice --port <PORT>              Start voice input mode
opencode-voice setup [--model <MODEL>]   Download whisper model
opencode-voice devices                   List available audio input devices
opencode-voice keys                      List available key names for --key / --hotkey
opencode-voice --help                    Show help
opencode-voice --version                 Show version
```

---

## Configuration

All options can be set via CLI flags or environment variables. CLI flags take precedence.

| Flag | Env Var | Default | Description |
|------|---------|---------|-------------|
| `--port <n>` | `OPENCODE_VOICE_PORT` | *(required)* | OpenCode server port |
| `--model <size>` | `OPENCODE_VOICE_MODEL` | `base.en` | Whisper model size |
| `--device <name>` | `OPENCODE_VOICE_DEVICE` | *(system default)* | Audio input device name |
| `--key <name>` | — | `space` | Toggle key for start/stop recording |
| `--hotkey <name>` | — | `right_option` | Global hotkey (system-wide, no terminal focus needed) |
| `--no-global` | — | — | Disable global hotkey support |
| `--push-to-talk` / `--no-push-to-talk` | — | `--push-to-talk` | Enable/disable push-to-talk mode |
| `--approval` / `--no-approval` | — | `--approval` | Review transcript before sending |

### Environment variables

| Variable | Description |
|----------|-------------|
| `OPENCODE_VOICE_PORT` | OpenCode server port (alternative to `--port`) |
| `OPENCODE_VOICE_MODEL` | Whisper model size (alternative to `--model`) |
| `OPENCODE_SERVER_PASSWORD` | Password if OpenCode server has auth enabled |

---

## How It Works

1. **Audio capture** — [cpal](https://github.com/RustAudio/cpal) captures audio from the microphone at 16 kHz mono
2. **Transcription** — [whisper-rs](https://github.com/tazz4843/whisper-rs) (whisper.cpp bindings) transcribes the captured audio entirely on-device
3. **Global hotkeys** — [rdev](https://github.com/Narsil/rdev) provides system-wide key event listening (no terminal focus required)
4. **Injection** — the transcribed text is sent via `POST /tui/append-prompt` to OpenCode's HTTP API, which inserts it into the active prompt textarea
5. **Approval** — when `--approval` is enabled, voice commands can respond to OpenCode permission and question prompts (e.g. "allow", "reject", "always")

---

## Platform Support

| Platform | Status | Notes |
|----------|--------|-------|
| macOS | Primary — fully tested | Requires Accessibility permission for global hotkeys |
| Linux | Supported | ALSA or PulseAudio via cpal |
| Windows | Not supported | — |

### macOS: Accessibility Permission

Global hotkeys (rdev) require Accessibility access on macOS:

```
System Settings → Privacy & Security → Accessibility → enable for Terminal / iTerm2
```

Without this permission, global hotkeys will not work. Use `--no-global` to disable them and rely on terminal keypresses only.

---

## Troubleshooting

### Build fails: "cmake not found"

```bash
brew install cmake        # macOS
sudo apt install cmake    # Ubuntu/Debian
```

### "Microphone permission denied" (macOS)

macOS requires explicit microphone permission for terminal applications:

```
System Settings → Privacy & Security → Microphone → enable for Terminal / iTerm2
```

### "Cannot connect to OpenCode"

Ensure OpenCode is running with the `--port` flag:

```bash
opencode --port 4096
```

Without `--port`, OpenCode does not expose an HTTP server and opencode-voice cannot connect.

### "Whisper model not downloaded"

Run the setup command to download the model:

```bash
opencode-voice setup
```

### "No speech detected"

- Speak closer to the microphone
- Try a larger model: `--model small.en`
- Reduce background noise
- Check available audio devices: `opencode-voice devices`

### Authentication errors (401)

If OpenCode is running with a server password, set it via environment variable:

```bash
export OPENCODE_SERVER_PASSWORD=your-password
opencode-voice --port 4096
```

---

## License

Apache 2.0 — see [LICENSE](LICENSE).
