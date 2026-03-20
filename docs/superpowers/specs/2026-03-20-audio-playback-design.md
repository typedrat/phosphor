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
    pub seek_target: AtomicUsize,   // usize::MAX = no pending seek
    pub playing: AtomicBool,
    pub looping: AtomicBool,
    pub speed: AtomicF32,
    pub volume: AtomicF32,
}
```

`position` is in frames (each frame = `channels` samples), so total frames = `samples.len() / channels`.

Play/pause, speed, loop, and volume are atomic reads/writes on this shared state. Seeking writes to `seek_target`; the cpal callback applies it at the start of each buffer (see below).

**Convenience methods** on `SharedAudioPlayback`:

- `total_frames() -> usize` — `samples.len() / channels as usize`
- `duration_secs() -> f32` — `total_frames() as f32 / sample_rate as f32`
- `position_secs() -> f32` — `position.load(Relaxed) as f32 / sample_rate as f32`
- `seek_to_frame(frame: usize)` — writes `seek_target.store(frame.min(total_frames()), Relaxed)`
- `seek_to_secs(secs: f32)` — converts to frame index, calls `seek_to_frame`

### Sample Storage

Decoded audio is stored as a flat `Arc<[f32]>` with interleaved stereo samples, matching symphonia's native output format. The sim thread accesses per-channel values by indexing `samples[frame * channels + channel]`.

### Audio Output (cpal + dasp_interpolate)

New module `src/audio_output.rs`. `AudioOutput` owns the cpal `Stream` and a reference to shared state:

```rust
pub struct AudioOutput {
    _stream: cpal::Stream,
    shared: Arc<SharedAudioPlayback>,
}
```

**Constructor is fallible.** `AudioOutput::new()` returns `anyhow::Result<Self>`. If no audio device is available (headless, broken PulseAudio/PipeWire/ALSA), the app degrades gracefully to visual-only mode — `App.audio_output` stays `None`, and the UI shows a warning but remains functional.

**The cpal callback** maintains callback-local state captured in the closure:

- `fractional_pos: f64` — sub-sample accumulator for resampling. The atomic `position` is updated to `fractional_pos.floor() as usize` after each output buffer.
- Uses **linear interpolation** via `dasp_interpolate::linear::Linear` — low CPU cost, appropriate for real-time audio callback. Sinc would be higher quality but risks underruns.

Callback logic per output buffer:

1. Check `seek_target`: if not `usize::MAX`, reset `fractional_pos` to that frame index, store `usize::MAX` back, and update `position`.
2. Read `playing`: if false, fill buffer with silence and return.
3. Read `speed`, `volume`.
4. Compute advance rate: `(source_sample_rate * speed) / device_sample_rate` source frames per output frame.
5. For each output frame: interpolate L/R from source samples at `fractional_pos`, apply volume, write to output buffer, advance `fractional_pos` by the advance rate.
6. On reaching end: if `looping`, wrap `fractional_pos` to 0; otherwise set `playing = false` and fill remaining output with silence.
7. Update atomic `position` to `fractional_pos.floor() as usize`.

Speed folds into the resampling ratio. At speed 2.0 with 44.1kHz source and 48kHz device, the advance rate is `(44100 * 2.0) / 48000 ~ 1.8375` source frames per output frame. Pitch changes proportionally — correct for oscilloscope music where the visual IS the audio.

### Sim Thread Changes

The sim thread no longer owns or advances audio position. `AudioState` holds `Option<Arc<SharedAudioPlayback>>` and a `last_audio_pos: usize`. In `generate_samples_fixed()`:

- Read the atomic `position` each batch
- If position differs from `last_audio_pos` in either direction (forward or backward), and the jump is larger than the expected batch delta, treat it as a seek/loop — reset `last_audio_pos` to the current position without generating beam samples for the skipped/rewound region.
- Otherwise, generate `BeamSample`s for samples in `[last_audio_pos, current_position)`.
- **L/R to X/Y mapping**: read `samples[frame * channels]` and `samples[frame * channels + 1]`, apply `(sample + 1.0) / 2.0` to map `[-1, 1]` to `[0, 1]` CRT coordinates. This mapping previously lived in `AudioSource::generate()` and moves here.
- Update `last_audio_pos = current_position`.

The existing `playing`, `looping`, `speed` fields on `AudioState` (sim-side) are removed. The sim thread reads these from the shared atomics. `AudioState` simplifies to:

```rust
pub struct AudioState {
    pub shared: Option<Arc<SharedAudioPlayback>>,
    pub last_audio_pos: usize,
}
```

### Background File Decoding

Decoding happens on a dedicated thread to avoid blocking the main thread:

1. UI sets `pending_file`
2. `App` spawns a decoding thread, stores the `Receiver<anyhow::Result<DecodedAudio>>` in `AudioUiState.decode_receiver`
3. Each frame, `App` polls `decode_receiver.try_recv()`
4. On success: creates `SharedAudioPlayback` + `AudioOutput` (fallible — visual-only if no audio device), stores `Arc` in `AudioUiState.shared`, sends clone to sim thread via `SimCommand::SetAudioShared`, starts playback
5. On error: sets `AudioUiState.load_error` (displayed as string via `{:#}`)
6. UI shows a loading indicator while `decode_receiver` is `Some` and hasn't resolved

```rust
pub struct DecodedAudio {
    pub samples: Arc<[f32]>,    // Interleaved stereo
    pub sample_rate: u32,
    pub channels: u16,
}
```

## UI

### AudioUiState Changes

With the shared atomic state, several `AudioUiState` fields become redundant. The struct simplifies:

```rust
pub struct AudioUiState {
    pub file_path: Option<PathBuf>,
    pub shared: Option<Arc<SharedAudioPlayback>>,   // The shared state (read atomics for display)
    pub decode_receiver: Option<Receiver<anyhow::Result<DecodedAudio>>>,
    pub load_error: Option<String>,
    pub pending_file: Option<PathBuf>,
}
```

`playing`, `looping`, `speed`, `has_file`, and `volume` are read from / written to `shared` atomics directly. `has_file` is replaced by `shared.is_some()`.

### Sidebar Audio Controls (enhanced)

Existing controls (file picker, play/pause, loop toggle, speed slider) plus:

- **Seek bar** — slider mapped to `0.0..=duration_secs`. On drag, calls `shared.seek_to_secs()`. Displays current position via `shared.position_secs()`.
- **Position / duration label** — `"1:23 / 4:56"` format, updated each frame from shared atomics
- **Volume slider** — `0.0..=1.0`, writes to `shared.volume`
- **Loading indicator** — shown while `decode_receiver` is `Some`

### Floating Media Overlay

New struct `MediaOverlay` in `src/ui/media_overlay.rs`. Rendered during the egui pass, positioned over the CRT viewport.

**Visibility:**

- Only when `input_mode == Audio` and `shared.is_some()`
- Detached mode: appears on mouse hover anywhere over the sim window
- Combined mode: appears on mouse hover over the viewport area (right of sidebar)
- Stays visible while mouse is over the overlay itself (anti-flicker)
- Fades in/out with opacity animation (~200ms ease-in-out)

**Viewport bounds:** `MediaOverlay::show()` takes an `egui::Rect` parameter for the viewport area. In combined mode, this is computed from `panel_width` and window size (already available in `UiState`). In detached mode, it's the full window rect.

**Layout:**

- `egui::Area` anchored to bottom-center of the viewport rect, ~40px tall, ~80% of viewport width
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

| Crate              | Purpose                                           |
| ------------------ | ------------------------------------------------- |
| `cpal`             | Audio output device/stream management             |
| `dasp_signal`      | `from_interleaved_samples_iter`, signal utilities |
| `dasp_interpolate` | Linear interpolation for sample rate conversion   |

Existing: `atomic_float` (already used for `SimStats`), `symphonia` (decode), `anyhow` (errors).

## File Changes

### New Files

| File                      | Contents                                                                                     |
| ------------------------- | -------------------------------------------------------------------------------------------- |
| `src/audio_output.rs`     | `AudioOutput`, `SharedAudioPlayback`, `DecodedAudio`, cpal stream setup, resampling callback |
| `src/ui/media_overlay.rs` | `MediaOverlay`, floating controls, fade animation                                            |

### Modified Files

| File                    | Changes                                                                                                                                                                               |
| ----------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `src/beam/audio.rs`     | Remove `AudioSource` struct. Keep symphonia decoding as a free function returning `DecodedAudio`.                                                                                     |
| `src/simulation.rs`     | `AudioState` simplifies to `shared` + `last_audio_pos`. Read position from shared state. L/R-to-X/Y mapping moves here. Remove old audio commands. Add `SetAudioShared`.              |
| `src/ui/mod.rs`         | `AudioUiState` simplifies: remove `playing`/`looping`/`speed`/`has_file`, add `shared`, `decode_receiver`.                                                                            |
| `src/ui/scope_panel.rs` | Add seek bar, position/duration label, volume slider. Read/write shared atomics instead of local fields.                                                                              |
| `src/frame.rs`          | Remove old audio command dispatch (play/pause/speed/loop). Audio file loading moves to `App`.                                                                                         |
| `src/app.rs`            | Add `audio_output: Option<AudioOutput>`. Spawn decode thread on `pending_file`. Poll decode receiver. Create `AudioOutput` + `SharedAudioPlayback` on completion. Send to sim thread. |
| `Cargo.toml`            | Add `cpal`, `dasp_signal`, `dasp_interpolate`.                                                                                                                                        |

### Deleted

- `AudioSource` struct (replaced by `SharedAudioPlayback` + free decode function)
- `SimCommand::LoadAudioFile`, `SetAudioPlaying`, `SetAudioLooping`, `SetAudioSpeed`
- `InputState::load_audio_file()` method
