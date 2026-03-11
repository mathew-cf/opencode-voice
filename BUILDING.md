# Building from source

## Prerequisites

### Rust toolchain (1.70+)

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

### cmake

Required to build whisper-rs (whisper.cpp bindings).

```bash
# macOS
brew install cmake

# Debian/Ubuntu
sudo apt install cmake

# Fedora
sudo dnf install cmake
```

### C compiler

- **macOS**: `xcode-select --install`
- **Linux**: `gcc` or `clang` (usually pre-installed)

### Linux system libraries

Audio capture and global hotkeys require development headers on Linux:

```bash
# Debian/Ubuntu
sudo apt install libasound2-dev libx11-dev libxi-dev libxtst-dev

# Fedora
sudo dnf install alsa-lib-devel libX11-devel libXi-devel libXtst-devel
```

| Package (Debian) | Purpose |
|-------------------|---------|
| `libasound2-dev` | Audio capture (cpal/ALSA) |
| `libx11-dev` | X11 display connection (rdev) |
| `libxi-dev` | Input device events (rdev) |
| `libxtst-dev` | Global hotkey capture (rdev) |

## Build

```bash
git clone https://github.com/mathew-cf/opencode-voice.git
cd opencode-voice
cargo build --release
```

The binary is at `target/release/opencode-voice`.

## Install from source

```bash
cargo install --path .
```

This installs to `~/.cargo/bin/` (should be on your `$PATH` after installing Rust).

## Troubleshooting

### "cmake not found"

```bash
brew install cmake        # macOS
sudo apt install cmake    # Debian/Ubuntu
```

### "system library `x11` was not found" (Linux)

```bash
sudo apt install libx11-dev libxi-dev libxtst-dev   # Debian/Ubuntu
sudo dnf install libX11-devel libXi-devel libXtst-devel  # Fedora
```

### "Microphone permission denied" (macOS)

```
System Settings → Privacy & Security → Microphone → enable for Terminal / iTerm2
```
