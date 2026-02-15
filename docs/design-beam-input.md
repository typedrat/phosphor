# Beam Model & Input System

## Electron Beam Physics

### Spot Profile

The electron beam produces a spatial intensity distribution on the phosphor screen, modeled as a Gaussian core plus a low-intensity halo from scattered electrons:

```
I(r) = (1 - h) · exp(-r² / 2σ²) + h · exp(-r² / 2σ_halo²)
```

Parameters:

- **σ (core width)**: Controlled by focus setting and acceleration voltage. Higher voltage → stiffer beam → smaller spot. Typical range: 0.5–3 pixels at screen resolution.
- **σ_halo**: ~3–5× the core width. Represents electrons scattered by gas molecules and aperture diffraction.
- **h (halo fraction)**: Small (~0.02–0.05). Fraction of total beam current in the halo.

### Beam Current Effects

- **Brightness**: Energy deposited per unit time is proportional to beam current. Higher current → brighter trace.
- **Space charge blooming**: At high current, mutual repulsion of electrons in the beam widens the spot: `σ_effective = σ_base · (1 + k · I_beam)`.
- **Saturation**: Phosphor has a finite capacity to convert electron energy to light. At extreme beam current, luminous efficiency drops. Modeled as a soft clamp on deposited energy.

### Acceleration Voltage

- Higher voltage → more kinetic energy per electron → brighter emission
- Higher voltage → smaller spot (stiffer beam, less deflection sensitivity)
- Voltage also affects penetration depth into the phosphor layer, which can shift the emission spectrum slightly (we may ignore this for v1)

## BeamSample — Common Input Format

All input modes produce a stream of:

```rust
struct BeamSample {
    x: f32,          // normalized [0, 1] horizontal position
    y: f32,          // normalized [0, 1] vertical position
    intensity: f32,  // beam current (0 = blanked)
    dt: f32,         // dwell time at this position (seconds)
}
```

The ring buffer between the input thread and the GPU is double-buffered: the input thread writes to one half while the main thread drains the other into a GPU staging buffer each frame.

## Input Mode: Oscilloscope

Built-in signal generators produce continuous X and Y voltage signals.

**Per-channel controls:**

- Waveform: sine, triangle, square, sawtooth, noise
- Frequency (Hz)
- Amplitude (normalized, maps to screen deflection)
- Phase (degrees)
- DC offset

**Global controls:**

- Simulated sample rate (e.g., 1M samples/sec)
- Timebase: ratio of simulated time to real time (1x = real-time, 10x = fast-forward, 0.1x = slow-motion)

Classic patterns emerge naturally: Lissajous figures from two sines at related frequencies, circles from sine/cosine, spirals from decaying amplitude, etc.

## Input Mode: Audio

Stereo audio file where left channel → X deflection, right channel → Y deflection.

**Mapping:** Audio samples are in [-1, 1]. Map to screen coordinates: `x = (L + 1) / 2`, `y = (R + 1) / 2`.

**Sample rate:** The audio file's native sample rate (typically 44.1kHz or 48kHz) defines the beam sample rate. Each audio sample becomes one BeamSample with `dt = 1 / sample_rate`.

**Beam intensity:** Constant (set by the UI's intensity knob). Optionally, a toggle to derive intensity from signal amplitude for artistic effects.

**Decoding:** symphonia handles WAV, FLAC, OGG/Vorbis, MP3.

**Playback controls:** Play, pause, seek, loop. Timebase/speed control for slow-motion viewing of individual waveform cycles.

**Future:** Real-time audio input from a system audio device (via CPAL) for live visualization. File playback is the priority for v1.

## Input Mode: Vector Graphics

Accepts a display list of line segments:

```rust
struct VectorSegment {
    x0: f32, y0: f32,  // start point (normalized)
    x1: f32, y1: f32,  // end point (normalized)
    intensity: f32,     // beam current for this segment
}
```

The beam traverses each segment in sequence. Between disconnected segments, the beam is blanked (intensity = 0) and repositioned — simulating real CRT retrace behavior with configurable settling time (deflection amplifier slew rate).

**Line subdivision:** Long segments are subdivided so that every pixel along the path receives appropriate energy. The dwell time per subdivision is `segment_length / (beam_speed * num_subdivisions)`. Slower sweep = more energy per pixel = brighter line.

**Display list sources:** Load from a JSON file, or accept via the external protocol.

## Input Mode: External

Text-based protocol over stdin or a Unix domain socket. Designed to be trivially drivable from any language.

```
B <x> <y> <intensity> <dt>           # single beam sample
L <x0> <y0> <x1> <y1> <intensity>   # line segment (subdivided internally)
F                                     # frame sync — flush current batch
```

All coordinates are normalized [0, 1]. Lines starting with `#` are comments. A binary protocol could be added later for higher throughput.

**Connection:** In stdin mode, reads from the process's stdin. In socket mode, listens on a configurable Unix domain socket path. Only one client at a time.
