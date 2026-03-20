# Audio Playback & Media Controls

## Problem

Audio input mode decodes files purely for X-Y beam coordinates. The user cannot hear the music being visualized, and playback controls are limited to play/pause, loop toggle, and speed slider — no seeking, no position display.

## Goals

1. Play audio through the system audio output, synchronized with the CRT beam visualization
2. Proper playback controls: seek bar, position/duration display, volume
3. Floating media overlay on the CRT viewport for quick access

## Architecture

### Audio-Authoritative Synchronization

The cpal audio output callback is the timing authority. It advances through the sample data at the hardware output rate, and the sim thread follows its position to generate beam samples. This guarantees that what you hear always matches what you see.

### Shared Playback State

A single `SharedAudioPlayback` struct behind an `Arc` is the coordination point between three consumers: the cpal callback, the sim thread, and the UI.

```rust
pub struct SharedAudioPlayback {
    pub samples: Arc<[f32]>,        // Interleaved stereo [L, R, L, R, ...]
    pub channels: u16,              // 2 for stereo
    pub sample_rate: u32,           // Source sample rate
    pub position: AtomicUsize,      // Current frame index (not sample index)
    pub playing: AtomicBool,
    pub looping: AtomicBool,
    pub speed: AtomicF32,
    pub volume: AtomicF32,
}
```

`position` is in frames (each frame = `channels` samples), so total frames = `samples.len() / channels`.

Play/pause, speed, loop, seek, and volume are all atomic reads/writes on this shared state. No command channel needed for these controls.

### Sample Storage

Decoded audio is stored as a flat `Arc<[f32]>` with interleaved stereo samples, matching symphonia's native output format. `dasp_signal::from_interleaved_samples_iter` provides per-channel access when needed for resampling.

### Audio Output (cpal + dasp_interpolate)

New module `src/audio_output.rs`. `AudioOutput` owns the cpal `Stream` and a reference to shared state:

```rust
pub struct AudioOutput {
    _stream: cpal::Stream,
    shared: Arc<SharedAudioPlayback>,
}
```

The cpal callback:

- Reads `playing`, `speed`, `volume` from shared state
- If not playing, fills output buffer with silence
- Otherwise, advances through samples at rate `(source_sample_rate * speed) / device_sample_rate` using `dasp_interpolate` for clean resampling
- Updates `position` atomically after each output buffer
- On reaching end: if `looping`, wraps position to 0; otherwise sets `playing = false`

Speed folds into the resampling ratio. At speed 2.0 with 44.1kHz source and 48kHz device, the advance rate is `(44100 * 2.0) / 48000 ~ 1.8375` source frames per output frame. Pitch changes proportionally — correct for oscilloscope music where the visual IS the audio.

### Sim Thread Changes

The sim thread no longer owns or advances audio position. `AudioState` holds `Option<Arc<SharedAudioPlayback>>`. In `generate_samples_fixed()`:

- Read the atomic position each batch
- Compute delta from `last_audio_pos` (new field)
- Generate `BeamSample`s for the new samples in that range
- If position jumped backwards (seek or loop), reset `last_audio_pos` — no beam samples for the gap

### Background File Decoding

Decoding happens on a dedicated thread to avoid blocking the main thread:

1. UI sets `pending_file`
2. Main thread spawns a decoding thread, stores a `Receiver<anyhow::Result<DecodedAudio>>`
3. Each frame, main thread calls `try_recv()` on the channel
4. On success: creates `SharedAudioPlayback` + `AudioOutput`, sends `Arc` to sim thread, starts playback
5. On error: sets `load_error` (displayed as string via `{:#}`)
6. UI shows a loading indicator while decode is in progress

```rust
pub struct DecodedAudio {
    pub samples: Arc<[f32]>,    // Interleaved stereo
    pub sample_rate: u32,
    pub channels: u16,
}
```

## UI

### Sidebar Audio Controls (enhanced)

Existing controls (file picker, play/pause, loop toggle, speed slider) plus:

- **Seek bar** — slider mapped to `0.0..=duration_secs`, reads/writes position from shared atomics
- **Position / duration label** — `"1:23 / 4:56"` format, updated each frame
- **Volume slider** — `0.0..=1.0`, writes to shared atomics
- **Loading indicator** — shown while background decode is in progress

### Floating Media Overlay

New struct `MediaOverlay` in `src/ui/media_overlay.rs`. Rendered during the egui pass, positioned over the CRT viewport.

**Visibility:**

- Only when `input_mode == Audio` and a file is loaded
- Detached mode: appears on mouse hover anywhere over the sim window
- Combined mode: appears on mouse hover over the viewport area (right of sidebar)
- Stays visible while mouse is over the overlay itself (anti-flicker)
- Fades in/out with opacity animation (~200ms ease-in-out)

**Layout:**

- `egui::Area` anchored to bottom-center of viewport, ~40px tall, ~80% of viewport width
- Semi-transparent dark background (`Color32::from_black_alpha(180)`)
- Single row: play/pause button, seek bar (remaining width), position/duration label
- Compact — no speed/volume/loop (those stay in sidebar only)

**State:**

```rust
pub struct MediaOverlay {
    opacity: f32,           // 0.0..=1.0, animated
    hover_active: bool,
}
```

## SimCommand Changes

- Remove: `LoadAudioFile`, `SetAudioPlaying`, `SetAudioLooping`, `SetAudioSpeed`
- Add: `SetAudioShared(Option<Arc<SharedAudioPlayback>>)`

Play/pause, speed, loop, and seek are all handled via atomic operations on the shared state. The only command needed is to give the sim thread its reference to the shared data when a new file is loaded.

## Dependencies

| Crate              | Purpose                                               |
| ------------------ | ----------------------------------------------------- |
| `cpal`             | Audio output device/stream management                 |
| `dasp_signal`      | `from_interleaved_samples_iter`, signal abstractions  |
| `dasp_interpolate` | Sample rate conversion (linear or sinc interpolation) |

Existing: `atomic_float` (already used for `SimStats`), `symphonia` (decode), `anyhow` (errors).

## File Changes

### New Files

| File                      | Contents                                                                                     |
| ------------------------- | -------------------------------------------------------------------------------------------- |
| `src/audio_output.rs`     | `AudioOutput`, `SharedAudioPlayback`, `DecodedAudio`, cpal stream setup, resampling callback |
| `src/ui/media_overlay.rs` | `MediaOverlay`, floating controls, fade animation                                            |

### Modified Files

| File                    | Changes                                                                                                                                                          |
| ----------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `src/beam/audio.rs`     | Remove `AudioSource` struct. Keep symphonia decoding as a free function returning `DecodedAudio`.                                                                |
| `src/simulation.rs`     | `AudioState` holds `Option<Arc<SharedAudioPlayback>>`. Read position from shared state. Remove old audio commands. Add `SetAudioShared`. Track `last_audio_pos`. |
| `src/ui/mod.rs`         | `AudioUiState` gains `shared`, `decode_receiver`, volume.                                                                                                        |
| `src/ui/scope_panel.rs` | Add seek bar, position/duration label, volume slider.                                                                                                            |
| `src/frame.rs`          | Poll decode receiver, create `AudioOutput` + shared state on completion, send to sim thread. Remove old audio command dispatch.                                  |
| `src/app.rs`            | Add `audio_output: Option<AudioOutput>`. Spawn decode thread on `pending_file`.                                                                                  |
| `Cargo.toml`            | Add `cpal`, `dasp_signal`, `dasp_interpolate`.                                                                                                                   |

### Deleted

- `AudioSource` struct (replaced by `SharedAudioPlayback` + free decode function)
- `SimCommand::LoadAudioFile`, `SetAudioPlaying`, `SetAudioLooping`, `SetAudioSpeed`
- `InputState::load_audio_file()` method
