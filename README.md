# Phosphor

A physically-based X-Y CRT simulator in Rust. Rather than approximating the visual appearance of a CRT, Phosphor models the actual physics of phosphor emission, decay, and electron beam behavior.

**Status:** Early development.

## Features

- **Spectral phosphor model** — 16-band spectral representation (380–780nm) with multi-exponential decay fitted to real Tektronix CRT data
- **Dual-layer phosphors** — Supports phosphors with distinct fluorescence and phosphorescence characteristics (P2, P7, P14, etc.)
- **GPU accumulation buffer** — Beam write, decay, and tonemapping run entirely on the GPU via wgpu compute/fragment shaders
- **Multiple input modes:**
  - Built-in oscilloscope signal generators (sine, triangle, square, sawtooth, noise)
  - Stereo audio files as X/Y input (oscilloscope music)
  - Vector display lists
  - External protocol over stdin/Unix socket
- **CRT display effects** — Bloom/halation, glass tint, screen curvature, edge falloff, filmic tonemapping

## Building

Requires Rust 2024 edition (1.85+) and GPU drivers with wgpu support.

```bash
cargo run --release
```

### Nix

A flake is provided for Linux:

```bash
nix run
```

## Input Modes

### Oscilloscope

Built-in waveform generators drive the X and Y axes. Useful for Lissajous figures and testing.

### Audio

Load a stereo audio file (WAV, FLAC, OGG, MP3) where the left channel drives X and the right channel drives Y — the format used by [oscilloscope music](https://oscilloscopemusic.com/).

### Vector

A display list of line segments with per-segment intensity control.

### External

A text protocol over stdin or Unix socket for driving the beam from external programs:

```
B x y intensity dt      # beam point
L x0 y0 x1 y1 intensity # line segment
F                        # end frame
```

## Phosphor Types

Phosphor definitions are based on the 1966 Tektronix CRT Data sheets (included in `docs/crt-info/`). Supported types include P1, P2, P7, P11, P31, and others.

## License

This project is licensed under the [Mozilla Public License 2.0](LICENSE).
