# opencode-voice

**Voice control for [OpenCode](https://opencode.ai)**

Everything runs on your machine — no cloud services, no API keys, no audio leaves your device.

```
Microphone → whisper.cpp → text → OpenCode
```

---

## Install

```bash
cargo install opencode-voice
```

> Requires Rust 1.70+, cmake, and a C compiler. See [Building from source](BUILDING.md) for details.

## Setup

Download a transcription model (~150 MB):

```bash
opencode-voice setup
```

## Run

Start OpenCode with its HTTP server, then start opencode-voice in a separate terminal:

```bash
opencode --port 4096
opencode-voice --port 4096
```

Hold **space** to record, release to transcribe and send. Press **q** to quit.

---

## Options

| Flag | Default | Description |
|------|---------|-------------|
| `--port <n>` | *(required)* | OpenCode server port |
| `--model <size>` | `base.en` | Whisper model (`tiny.en`, `base.en`, `small.en`) |
| `--device <name>` | system default | Audio input device |
| `--key <name>` | `space` | Toggle key for recording |
| `--hotkey <name>` | `right_option` | Global hotkey (system-wide) |
| `--no-global` | — | Disable global hotkey |
| `--no-push-to-talk` | — | Toggle mode instead of hold-to-talk |
| `--no-approval` | — | Skip transcript review before sending |

Environment variables: `OPENCODE_VOICE_PORT`, `OPENCODE_VOICE_MODEL`, `OPENCODE_SERVER_PASSWORD`.

## Other commands

```
opencode-voice setup [--model <MODEL>]   Download whisper model
opencode-voice devices                   List audio input devices
opencode-voice keys                      List key names for --key / --hotkey
```

## Troubleshooting

| Problem | Fix |
|---------|-----|
| Can't connect to OpenCode | Make sure OpenCode is running with `--port` |
| No speech detected | Speak closer to mic, try `--model small.en`, check `opencode-voice devices` |
| Whisper model not found | Run `opencode-voice setup` |
| Global hotkeys don't work (macOS) | Grant Accessibility permission in System Settings, or use `--no-global` |
| Auth errors (401) | Set `OPENCODE_SERVER_PASSWORD` env var |
| Build failures | See [Building from source](BUILDING.md) |

## Platform support

| Platform | Status |
|----------|--------|
| macOS | Fully supported (requires Accessibility permission for global hotkeys) |
| Linux | Supported (ALSA or PulseAudio) |
| Windows | Not supported |

## License

Apache 2.0 — see [LICENSE](LICENSE).
