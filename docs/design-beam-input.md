# Beam Model & Input System

## Electron Beam Physics

### Spot Profile

The electron beam produces a spatial intensity distribution on the phosphor screen, modeled as a Gaussian core plus a low-intensity halo from scattered electrons:

```
I(r) = (1 - h) · exp(-r² / 2σ²) + h · exp(-r² / 2σ_halo²)
```

Parameters:

- **σ (core width)**: Controlled by focus setting and acceleration voltage. Higher voltage → stiffer beam → smaller spot. Typical range: 0.5–5 pixels at screen resolution.
- **σ_halo**: ~3–5× the core width. Represents electrons scattered by gas molecules and aperture diffraction.
- **h (halo fraction)**: Small (~0.02–0.05). Fraction of total beam current in the halo.

### Beam Current Effects

- **Brightness**: Energy deposited per unit time is proportional to beam current. Higher current → brighter trace.
- **Space charge blooming**: At high current, mutual repulsion of electrons in the beam widens the spot: `σ_effective = σ_base · (1 + k · I_beam)`.

### Acceleration Voltage

- Higher voltage → more kinetic energy per electron → brighter emission
- Higher voltage → smaller spot (stiffer beam, less deflection sensitivity)

## BeamSample — Common Input Format

All input modes produce a stream of:

```rust
#[repr(C)]
struct BeamSample {
    x: f32,          // normalized [0, 1] horizontal position
    y: f32,          // normalized [0, 1] vertical position
    intensity: f32,  // beam current (0 = blanked)
    dt: f32,         // dwell time at this position (seconds)
}
```

The struct derives `bytemuck::Pod` for zero-copy upload to a GPU storage buffer.

## Arc-Length Resampling

At high sample rates, consecutive samples are closer together than the beam radius, creating visible periodic brightness modulation along traces. Before uploading to the GPU, the CPU resamples the beam path by arc length:

- Merge short segments into longer ones, spacing depositions at ~0.5× beam sigma intervals
- Energy is conserved: merged segments' `intensity × dt` equals the sum of constituent samples
- First lit sample in each run emits directly (line-start anchor); subsequent depositions emit when accumulated arc length exceeds the threshold
- Remaining energy is flushed at the end

This decouples energy deposition rate from input sample rate, producing uniform trace brightness regardless of source.

## Aspect Ratio Correction

Beam coordinates are corrected for display aspect ratio before GPU upload: the wider axis is compressed around center so that equal deflection amplitudes produce equal physical distances on screen. A sine/cosine Lissajous appears as a circle, not an ellipse.

## Input Mode: Oscilloscope

Built-in signal generators produce continuous X and Y voltage signals.

**Per-channel controls:**

- Waveform: sine, triangle, square, sawtooth, noise
- Frequency (Hz)
- Amplitude (normalized, maps to screen deflection)
- Phase (radians)
- DC offset

**Global controls:**

- Sample rate (default 44100 Hz)

Classic patterns emerge naturally: Lissajous figures from two sines at related frequencies, circles from sine/cosine, spirals from decaying amplitude, etc.

## Input Mode: Audio

Stereo audio file where left channel → X deflection, right channel → Y deflection.

**Mapping:** Audio samples are in [-1, 1]. Map to screen coordinates: `x = (L + 1) / 2`, `y = (R + 1) / 2`.

**Sample rate:** The audio file's native sample rate (typically 44.1kHz or 48kHz) defines the beam sample rate. Each audio sample becomes one BeamSample with `dt = 1 / sample_rate`.

**Decoding:** symphonia handles WAV, FLAC, OGG/Vorbis, MP3.

**Playback controls:** Play, pause, loop, speed control, file picker via native dialog (rfd).

## Input Mode: Vector Graphics

Accepts a display list of line segments loaded from JSON:

```rust
struct VectorSegment {
    x0: f32, y0: f32,  // start point (normalized)
    x1: f32, y1: f32,  // end point (normalized)
    intensity: f32,     // beam current for this segment
}
```

The beam traverses each segment in sequence. Between disconnected segments, the beam is blanked (intensity = 0) and repositioned — simulating real CRT retrace behavior with configurable settling time.

**Line subdivision:** Long segments are subdivided so that every pixel along the path receives appropriate energy, based on the beam spot radius.

## Input Mode: External

Text-based protocol over stdin or a Unix domain socket. Designed to be trivially drivable from any language. Parsed with nom.

```
B <x> <y> <intensity> <dt>           # single beam sample
L <x0> <y0> <x1> <y1> <intensity>   # line segment (subdivided internally)
F                                     # frame sync — flush current batch
```

All coordinates are normalized [0, 1]. Lines starting with `#` are comments.
