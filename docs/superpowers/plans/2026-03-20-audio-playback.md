# Audio Playback & Media Controls Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add system audio output synchronized with the CRT beam visualization, plus enhanced playback controls and a floating media overlay.

**Architecture:** Shared atomic state (`SharedAudioPlayback`) coordinates three consumers: cpal audio output callback (timing authority), sim thread (follows audio position for beam generation), and UI (reads position for display, writes for seek/controls). Background decoding on a dedicated thread keeps the main thread responsive.

**Tech Stack:** cpal (audio output), dasp_interpolate (resampling), dasp_signal (interleaved sample utilities), symphonia (existing decode)

**Spec:** `docs/superpowers/specs/2026-03-20-audio-playback-design.md`

---

### Task 1: Add dependencies

**Files:**

- Modify: `Cargo.toml`

- [ ] **Step 1: Add cpal, dasp_signal, and dasp_interpolate to Cargo.toml**

Add to the `[dependencies]` section after the symphonia block:

```toml
# Audio output
cpal = "0.15"

# Audio DSP (resampling, signal utilities)
dasp_signal = "0.11"
dasp_interpolate = { version = "0.11", features = ["linear"] }
```

- [ ] **Step 2: Verify the project compiles**

Run: `cargo check`
Expected: Compiles with no errors (new deps are unused but that's fine)

- [ ] **Step 3: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "deps: add cpal, dasp_signal, dasp_interpolate for audio output"
```

---

### Task 2: SharedAudioPlayback, DecodedAudio, and decode function

**Files:**

- Create: `src/audio_output.rs`
- Modify: `src/beam/audio.rs`
- Modify: `src/main.rs`

- [ ] **Step 1: Write tests for SharedAudioPlayback convenience methods**

Create `src/audio_output.rs` with the `SharedAudioPlayback` struct, `DecodedAudio` struct, and tests:

```rust
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use atomic_float::AtomicF32;

/// Decoded audio ready to be shared.
pub struct DecodedAudio {
    pub samples: Arc<[f32]>,
    pub sample_rate: u32,
    pub channels: u16,
}

/// Shared playback state coordinating cpal callback, sim thread, and UI.
pub struct SharedAudioPlayback {
    pub samples: Arc<[f32]>,
    pub channels: u16,
    pub sample_rate: u32,
    pub position: AtomicUsize,
    pub seek_target: AtomicUsize,
    pub playing: AtomicBool,
    pub looping: AtomicBool,
    pub speed: AtomicF32,
    pub volume: AtomicF32,
}

impl SharedAudioPlayback {
    pub fn new(decoded: DecodedAudio) -> Self {
        Self {
            samples: decoded.samples,
            channels: decoded.channels,
            sample_rate: decoded.sample_rate,
            position: AtomicUsize::new(0),
            seek_target: AtomicUsize::new(usize::MAX),
            playing: AtomicBool::new(false),
            looping: AtomicBool::new(false),
            speed: AtomicF32::new(1.0),
            volume: AtomicF32::new(1.0),
        }
    }

    pub fn total_frames(&self) -> usize {
        self.samples.len() / self.channels as usize
    }

    pub fn duration_secs(&self) -> f32 {
        self.total_frames() as f32 / self.sample_rate as f32
    }

    pub fn position_secs(&self) -> f32 {
        self.position.load(Ordering::Relaxed) as f32 / self.sample_rate as f32
    }

    pub fn seek_to_frame(&self, frame: usize) {
        self.seek_target
            .store(frame.min(self.total_frames()), Ordering::Relaxed);
    }

    pub fn seek_to_secs(&self, secs: f32) {
        let frame = (secs * self.sample_rate as f32) as usize;
        self.seek_to_frame(frame);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_shared(num_frames: usize, sample_rate: u32) -> SharedAudioPlayback {
        let channels: u16 = 2;
        let samples: Vec<f32> = vec![0.0; num_frames * channels as usize];
        SharedAudioPlayback::new(DecodedAudio {
            samples: samples.into(),
            sample_rate,
            channels,
        })
    }

    #[test]
    fn total_frames_stereo() {
        let shared = make_shared(1000, 44100);
        assert_eq!(shared.total_frames(), 1000);
    }

    #[test]
    fn duration_secs() {
        let shared = make_shared(44100, 44100);
        assert!((shared.duration_secs() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn position_secs_tracks_position() {
        let shared = make_shared(44100, 44100);
        shared.position.store(22050, Ordering::Relaxed);
        assert!((shared.position_secs() - 0.5).abs() < 1e-4);
    }

    #[test]
    fn seek_to_frame_clamps() {
        let shared = make_shared(1000, 44100);
        shared.seek_to_frame(5000);
        assert_eq!(shared.seek_target.load(Ordering::Relaxed), 1000);
    }

    #[test]
    fn seek_to_secs() {
        let shared = make_shared(44100, 44100);
        shared.seek_to_secs(0.5);
        assert_eq!(shared.seek_target.load(Ordering::Relaxed), 22050);
    }
}
```

- [ ] **Step 2: Register the module in main.rs**

Add `mod audio_output;` to `src/main.rs`.

- [ ] **Step 3: Run tests to verify they pass**

Run: `cargo test audio_output`
Expected: All 5 tests pass

- [ ] **Step 4: Add decode_audio_file function to beam/audio.rs (keep AudioSource temporarily)**

Add the new `decode_audio_file` free function to `src/beam/audio.rs` alongside the existing `AudioSource`. `AudioSource` will be removed in Task 4 when `simulation.rs` is updated. Add `use crate::audio_output::DecodedAudio;` to the imports.

```rust
use std::fs::File;
use std::path::Path;
use std::sync::Arc;

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

use crate::audio_output::DecodedAudio;

/// Decode an audio file into interleaved stereo samples.
pub fn decode_audio_file(path: &Path) -> anyhow::Result<DecodedAudio> {
    let file = File::open(path)?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    let probed = symphonia::default::get_probe().format(
        &hint,
        mss,
        &FormatOptions::default(),
        &MetadataOptions::default(),
    )?;

    let mut format = probed.format;
    let track = format
        .default_track()
        .ok_or_else(|| anyhow::anyhow!("no audio track found"))?;

    let sample_rate = track
        .codec_params
        .sample_rate
        .ok_or_else(|| anyhow::anyhow!("unknown sample rate"))?;
    let channels = track.codec_params.channels.map(|c| c.count()).unwrap_or(2);
    let track_id = track.id;

    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())?;

    let mut interleaved = Vec::new();

    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(symphonia::core::errors::Error::IoError(e))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break;
            }
            Err(e) => return Err(e.into()),
        };

        if packet.track_id() != track_id {
            continue;
        }

        let decoded = decoder.decode(&packet)?;
        let spec = *decoded.spec();
        let num_frames = decoded.capacity();

        let mut sample_buf = SampleBuffer::<f32>::new(num_frames as u64, spec);
        sample_buf.copy_interleaved_ref(decoded);

        interleaved.extend_from_slice(sample_buf.samples());
    }

    // Ensure stereo output: mono → duplicate, >2ch → take first two
    let stereo_interleaved: Vec<f32> = match channels {
        1 => interleaved
            .iter()
            .flat_map(|&s| [s, s])
            .collect(),
        2 => interleaved,
        n => interleaved
            .chunks_exact(n)
            .flat_map(|c| [c[0], c[1]])
            .collect(),
    };

    Ok(DecodedAudio {
        samples: Arc::from(stereo_interleaved),
        sample_rate,
        channels: 2, // Always stereo after conversion above
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Create a minimal WAV file with known content (IEEE float, stereo).
    fn make_test_wav(samples: &[(f32, f32)], sample_rate: u32) -> Vec<u8> {
        let num_samples = samples.len() as u32;
        let data_size = num_samples * 2 * 4;
        let file_size = 36 + data_size;

        let mut buf = Vec::new();
        buf.extend_from_slice(b"RIFF");
        buf.extend_from_slice(&file_size.to_le_bytes());
        buf.extend_from_slice(b"WAVE");
        buf.extend_from_slice(b"fmt ");
        buf.extend_from_slice(&16u32.to_le_bytes());
        buf.extend_from_slice(&3u16.to_le_bytes()); // IEEE float
        buf.extend_from_slice(&2u16.to_le_bytes()); // stereo
        buf.extend_from_slice(&sample_rate.to_le_bytes());
        buf.extend_from_slice(&(sample_rate * 2 * 4).to_le_bytes());
        buf.extend_from_slice(&8u16.to_le_bytes());
        buf.extend_from_slice(&32u16.to_le_bytes());
        buf.extend_from_slice(b"data");
        buf.extend_from_slice(&data_size.to_le_bytes());
        for (l, r) in samples {
            buf.extend_from_slice(&l.to_le_bytes());
            buf.extend_from_slice(&r.to_le_bytes());
        }
        buf
    }

    #[test]
    fn decode_stereo_wav() {
        let test_samples = vec![(0.5, -0.5), (1.0, -1.0), (-1.0, 1.0)];
        let wav = make_test_wav(&test_samples, 44100);
        let tmp = std::env::temp_dir().join("phosphor_test_decode.wav");
        std::fs::write(&tmp, &wav).unwrap();

        let decoded = decode_audio_file(&tmp).unwrap();
        assert_eq!(decoded.channels, 2);
        assert_eq!(decoded.sample_rate, 44100);
        // 3 frames × 2 channels = 6 samples
        assert_eq!(decoded.samples.len(), 6);
        assert!((decoded.samples[0] - 0.5).abs() < 0.01);  // L0
        assert!((decoded.samples[1] - -0.5).abs() < 0.01); // R0
        assert!((decoded.samples[2] - 1.0).abs() < 0.01);  // L1
        assert!((decoded.samples[3] - -1.0).abs() < 0.01); // R1

        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn decode_sample_rate() {
        let silence = vec![(0.0, 0.0); 100];
        let wav = make_test_wav(&silence, 48000);
        let tmp = std::env::temp_dir().join("phosphor_test_decode_rate.wav");
        std::fs::write(&tmp, &wav).unwrap();

        let decoded = decode_audio_file(&tmp).unwrap();
        assert_eq!(decoded.sample_rate, 48000);

        std::fs::remove_file(&tmp).ok();
    }
}
```

- [ ] **Step 5: Run all tests to verify nothing is broken**

Run: `cargo test`
Expected: All tests pass (AudioSource is still present so existing code compiles).

- [ ] **Step 6: Commit**

```bash
git add src/audio_output.rs src/beam/audio.rs src/main.rs
git commit -m "feat: add SharedAudioPlayback, DecodedAudio, and decode function

Add shared atomic playback state and interleaved decode function.
AudioSource kept temporarily — removed in next task."
```

---

### Task 3: AudioOutput — cpal stream with resampling

**Files:**

- Modify: `src/audio_output.rs`

- [ ] **Step 1: Add AudioOutput struct and constructor**

Add to `src/audio_output.rs` after the existing code, before the `#[cfg(test)]` module:

```rust
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use dasp_interpolate::linear::Linear;
use dasp_interpolate::Interpolator;

/// Manages the cpal output stream for audio playback.
pub struct AudioOutput {
    _stream: cpal::Stream,
    pub shared: Arc<SharedAudioPlayback>,
}

impl AudioOutput {
    /// Create a new audio output stream. Returns `Err` if no audio device
    /// is available — the app should degrade to visual-only mode.
    pub fn new(shared: Arc<SharedAudioPlayback>) -> anyhow::Result<Self> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| anyhow::anyhow!("no audio output device found"))?;

        let config = device.default_output_config()?;
        let device_sample_rate = config.sample_rate().0;
        let device_channels = config.channels() as usize;

        tracing::info!(
            device = device.name().unwrap_or_default(),
            sample_rate = device_sample_rate,
            channels = device_channels,
            "Audio output device"
        );

        let shared_cb = Arc::clone(&shared);
        let source_rate = shared.sample_rate;
        let source_channels = shared.channels as usize;

        // Callback-local state
        let mut fractional_pos: f64 = 0.0;
        let mut interp_l = Linear::new([0.0f32], [0.0f32]);
        let mut interp_r = Linear::new([0.0f32], [0.0f32]);
        let mut last_int_pos: usize = usize::MAX; // force initial load

        let stream = device.build_output_stream(
            &config.into(),
            move |data: &mut [f32], _info: &cpal::OutputCallbackInfo| {
                // Check for pending seek
                let seek = shared_cb.seek_target.swap(usize::MAX, Ordering::Relaxed);
                if seek != usize::MAX {
                    fractional_pos = seek as f64;
                    shared_cb.position.store(seek, Ordering::Relaxed);
                    last_int_pos = usize::MAX; // force interpolator reload
                }

                if !shared_cb.playing.load(Ordering::Relaxed) {
                    data.fill(0.0);
                    return;
                }

                let speed = shared_cb.speed.load(Ordering::Relaxed) as f64;
                let volume = shared_cb.volume.load(Ordering::Relaxed);
                let advance_rate =
                    (source_rate as f64 * speed) / device_sample_rate as f64;
                let total_frames = shared_cb.total_frames();
                let samples = &shared_cb.samples;

                let output_frames = data.len() / device_channels;
                for frame_idx in 0..output_frames {
                    let int_pos = fractional_pos as usize;

                    if int_pos >= total_frames {
                        if shared_cb.looping.load(Ordering::Relaxed) {
                            fractional_pos = 0.0;
                            last_int_pos = usize::MAX;
                        } else {
                            let start = frame_idx * device_channels;
                            data[start..].fill(0.0);
                            shared_cb.playing.store(false, Ordering::Relaxed);
                            break;
                        }
                    }

                    let pos = fractional_pos as usize;
                    let frac = fractional_pos - pos as f64;

                    // Feed new frames to the interpolators when we cross a frame boundary
                    if pos != last_int_pos {
                        let next_pos = if pos + 1 < total_frames {
                            pos + 1
                        } else if shared_cb.looping.load(Ordering::Relaxed) {
                            0
                        } else {
                            pos
                        };
                        let base = pos * source_channels;
                        let next_base = next_pos * source_channels;
                        interp_l = Linear::new(
                            [samples[base]],
                            [samples[next_base]],
                        );
                        interp_r = Linear::new(
                            [samples[base + 1]],
                            [samples[next_base + 1]],
                        );
                        last_int_pos = pos;
                    }

                    let [l] = interp_l.interpolate(frac);
                    let [r] = interp_r.interpolate(frac);

                    // Write to output — map stereo to device channels
                    let out_base = frame_idx * device_channels;
                    data[out_base] = l * volume;
                    if device_channels >= 2 {
                        data[out_base + 1] = r * volume;
                    }
                    for ch in 2..device_channels {
                        data[out_base + ch] = 0.0;
                    }

                    fractional_pos += advance_rate;
                }

                shared_cb
                    .position
                    .store(fractional_pos as usize, Ordering::Relaxed);
            },
            |err| {
                tracing::error!("Audio output error: {err}");
            },
            None,
        )?;

        stream.play()?;

        Ok(Self {
            _stream: stream,
            shared,
        })
    }
}
```

- [ ] **Step 2: Verify the project compiles**

Run: `cargo check`
Expected: Compiles (AudioOutput is defined but not yet used from app.rs)

- [ ] **Step 3: Commit**

```bash
git add src/audio_output.rs
git commit -m "feat: add AudioOutput with cpal stream and linear resampling

Real-time audio callback with fractional position tracking, seek
support, speed/volume/looping via shared atomics, and automatic
sample rate conversion between source and device rates."
```

---

### Task 4: Update simulation thread — audio-authoritative sync

**Files:**

- Modify: `src/simulation.rs`

- [ ] **Step 1: Update AudioState and SimCommand**

In `src/simulation.rs`:

Replace the `AudioState` struct (lines 24-44) with:

```rust
pub struct AudioState {
    pub shared: Option<Arc<SharedAudioPlayback>>,
    pub last_audio_pos: usize,
}

impl Default for AudioState {
    fn default() -> Self {
        Self {
            shared: None,
            last_audio_pos: 0,
        }
    }
}
```

Add import at the top:

```rust
use crate::audio_output::SharedAudioPlayback;
```

Remove import of `AudioSource`:

```rust
// Remove: use crate::beam::audio::AudioSource;
```

In the `SimCommand` enum, replace the four audio commands:

```rust
// Remove these:
// LoadAudioFile(PathBuf),
// SetAudioPlaying(bool),
// SetAudioLooping(bool),
// SetAudioSpeed(f32),

// Add this:
SetAudioShared(Option<Arc<SharedAudioPlayback>>),
```

Remove `PathBuf` from the imports if no longer needed (check if `LoadVectorFile` still uses it — it does, so keep it).

- [ ] **Step 2: Update SimState::apply_command**

Replace the four audio command arms in `apply_command` with:

```rust
SimCommand::SetAudioShared(shared) => {
    self.input.audio.last_audio_pos = shared
        .as_ref()
        .map_or(0, |s| s.position.load(Ordering::Relaxed));
    self.input.audio.shared = shared;
}
```

Remove the old arms:

```rust
// Remove:
// SimCommand::LoadAudioFile(path) => self.input.load_audio_file(path),
// SimCommand::SetAudioPlaying(p) => self.input.audio.playing = p,
// SimCommand::SetAudioLooping(l) => self.input.audio.looping = l,
// SimCommand::SetAudioSpeed(s) => self.input.audio.speed = s,
```

- [ ] **Step 3: Update generate_samples_fixed for audio-authoritative sync**

Replace the `InputMode::Audio` arm in `generate_samples_fixed` (lines 133-154) with:

```rust
InputMode::Audio => {
    let audio = &mut self.audio;
    let Some(shared) = &audio.shared else {
        return Vec::new();
    };
    if !shared.playing.load(Ordering::Relaxed) {
        return Vec::new();
    }

    let current_pos = shared.position.load(Ordering::Relaxed);
    let last_pos = audio.last_audio_pos;
    let channels = shared.channels as usize;
    let dt = 1.0 / shared.sample_rate as f32;

    // Detect discontinuity (seek or loop): position jumped backwards,
    // or jumped forward by more than ~100ms of audio. On discontinuity,
    // snap to the new position — don't generate samples for the gap.
    let max_reasonable_delta =
        (shared.sample_rate as f64 * 0.1) as usize; // ~100ms
    let is_discontinuity = current_pos < last_pos
        || current_pos.wrapping_sub(last_pos) > max_reasonable_delta;

    if is_discontinuity {
        audio.last_audio_pos = current_pos;
        return Vec::new();
    }

    // Generate beam samples for [last_pos, current_pos)
    let delta = current_pos - last_pos;
    let samples_data = &shared.samples;
    let mut result = Vec::with_capacity(delta);
    for frame in last_pos..current_pos {
        let idx = frame * channels;
        if idx + 1 >= samples_data.len() {
            break;
        }
        let l = samples_data[idx];
        let r = samples_data[idx + 1];
        result.push(BeamSample {
            x: (l + 1.0) / 2.0,
            y: (r + 1.0) / 2.0,
            intensity: 1.0,
            dt,
        });
    }

    audio.last_audio_pos = current_pos;
    result
}
```

- [ ] **Step 4: Remove load_audio_file method**

Delete the `load_audio_file` method from `InputState` (lines 206-219).

- [ ] **Step 5: Remove AudioSource from beam/audio.rs**

Now that `simulation.rs` no longer references `AudioSource`, remove it from `src/beam/audio.rs`. Delete the `AudioSource` struct, its `BeamSource` impl, and the old tests (keep `decode_audio_file`, `make_test_wav`, and the `decode_*` tests). Remove unused imports (`super::{BeamSample, BeamSource, BeamState}`).

- [ ] **Step 6: Verify compilation**

Run: `cargo check`
Expected: Compiles. There will be errors in `frame.rs` from the removed SimCommand variants — that's fixed in Task 5.

If `cargo check` fails due to `frame.rs`, that's expected. Verify the error messages are only from `frame.rs`.

- [ ] **Step 7: Commit**

```bash
git add src/simulation.rs src/beam/audio.rs
git commit -m "refactor: audio-authoritative sim thread sync

Sim thread follows shared atomic position from cpal callback instead
of advancing its own AudioSource. L/R-to-X/Y mapping moves here.
Remove AudioSource, LoadAudioFile, SetAudioPlaying/Looping/Speed."
```

---

### Task 5: Update AudioUiState and frame dispatch

**Files:**

- Modify: `src/ui/mod.rs`
- Modify: `src/frame.rs`

- [ ] **Step 1: Update AudioUiState**

In `src/ui/mod.rs`, replace the `AudioUiState` struct and its `Default` impl with:

```rust
use crate::audio_output::SharedAudioPlayback;

/// UI-only audio state. Playback controls (playing, looping, speed, volume)
/// are read/written directly on SharedAudioPlayback atomics.
pub struct AudioUiState {
    pub file_path: Option<PathBuf>,
    pub shared: Option<Arc<SharedAudioPlayback>>,
    pub decode_receiver: Option<crossbeam_channel::Receiver<anyhow::Result<crate::audio_output::DecodedAudio>>>,
    pub load_error: Option<String>,
    pub pending_file: Option<PathBuf>,
}

impl Default for AudioUiState {
    fn default() -> Self {
        Self {
            file_path: None,
            shared: None,
            decode_receiver: None,
            load_error: None,
            pending_file: None,
        }
    }
}
```

- [ ] **Step 2: Update frame.rs — remove old audio dispatch**

In `src/frame.rs`, remove the audio command dispatch lines (62-70):

```rust
// Remove these lines:
// let _ = tx.send(SimCommand::SetAudioPlaying(ui.audio_ui.playing));
// let _ = tx.send(SimCommand::SetAudioLooping(ui.audio_ui.looping));
// let _ = tx.send(SimCommand::SetAudioSpeed(ui.audio_ui.speed));
// if let Some(path) = ui.audio_ui.pending_file.take() {
//     ui.audio_ui.file_path = Some(path.clone());
//     ui.audio_ui.has_file = true;
//     let _ = tx.send(SimCommand::LoadAudioFile(path));
// }
```

The `pending_file` consumption and decode thread spawning will be handled by `App` in Task 6.

- [ ] **Step 3: Verify compilation**

Run: `cargo check`
Expected: Compiles. Warnings about unused fields are fine at this stage.

- [ ] **Step 4: Commit**

```bash
git add src/ui/mod.rs src/frame.rs
git commit -m "refactor: simplify AudioUiState, remove old audio dispatch

AudioUiState now holds Arc<SharedAudioPlayback> for atomic control.
Remove redundant playing/looping/speed/has_file fields.
Remove audio command dispatch from frame.rs — moves to App."
```

---

### Task 6: Wire audio output into App — decode thread, polling, lifecycle

**Files:**

- Modify: `src/app.rs`

- [ ] **Step 1: Add audio_output field and decode polling to App**

Add field to `App` struct:

```rust
audio_output: Option<crate::audio_output::AudioOutput>,
```

Initialize to `None` in `Default`.

Add import at the top of `app.rs`:

```rust
use crate::audio_output::{AudioOutput, SharedAudioPlayback};
```

- [ ] **Step 2: Add method to spawn decode thread**

Add a method to `App`:

```rust
fn spawn_audio_decode(&mut self) {
    let Some(ui) = &mut self.ui else { return };
    let Some(path) = ui.audio_ui.pending_file.take() else {
        return;
    };

    ui.audio_ui.file_path = Some(path.clone());
    ui.audio_ui.load_error = None;

    let (tx, rx) = crossbeam_channel::bounded(1);
    ui.audio_ui.decode_receiver = Some(rx);

    std::thread::Builder::new()
        .name("audio-decode".into())
        .spawn(move || {
            let result = crate::beam::audio::decode_audio_file(&path);
            let _ = tx.send(result);
        })
        .expect("failed to spawn audio decode thread");
}
```

- [ ] **Step 3: Add method to poll decode result**

Add a method to `App`:

```rust
fn poll_audio_decode(&mut self) {
    let Some(ui) = &mut self.ui else { return };
    let Some(rx) = &ui.audio_ui.decode_receiver else {
        return;
    };

    match rx.try_recv() {
        Ok(Ok(decoded)) => {
            let shared = Arc::new(SharedAudioPlayback::new(decoded));
            shared.playing.store(true, std::sync::atomic::Ordering::Relaxed);
            shared.looping.store(true, std::sync::atomic::Ordering::Relaxed);

            // Create audio output (fallible — degrade to visual-only)
            match AudioOutput::new(Arc::clone(&shared)) {
                Ok(output) => {
                    self.audio_output = Some(output);
                }
                Err(e) => {
                    tracing::warn!("No audio output: {e:#}");
                    ui.audio_ui.load_error =
                        Some(format!("Audio output unavailable: {e:#}"));
                }
            }

            // Send shared state to sim thread
            if let Some(tx) = &self.sim_commands {
                let _ = tx.send(SimCommand::SetAudioShared(Some(Arc::clone(&shared))));
            }

            ui.audio_ui.shared = Some(shared);
            ui.audio_ui.decode_receiver = None;
        }
        Ok(Err(e)) => {
            ui.audio_ui.load_error = Some(format!("{e:#}"));
            ui.audio_ui.decode_receiver = None;
        }
        Err(crossbeam_channel::TryRecvError::Empty) => {
            // Still decoding
        }
        Err(crossbeam_channel::TryRecvError::Disconnected) => {
            ui.audio_ui.load_error = Some("Decode thread crashed".to_string());
            ui.audio_ui.decode_receiver = None;
        }
    }
}
```

- [ ] **Step 4: Call from RedrawRequested**

In `handle_viewport_event`, inside the `WindowEvent::RedrawRequested` arm, add these calls before the `dispatch_sim_commands` call:

```rust
// Audio file loading (background decode)
self.spawn_audio_decode();
self.poll_audio_decode();
```

- [ ] **Step 5: Add SimCommand import and shutdown cleanup**

Make sure `SimCommand` is imported. In the `CloseRequested` handler, drop the audio output before shutting down:

```rust
// In CloseRequested, before sending Shutdown:
self.audio_output = None;
```

- [ ] **Step 6: Verify compilation**

Run: `cargo check`
Expected: Compiles with possible warnings about unused imports.

- [ ] **Step 7: Run tests**

Run: `cargo test`
Expected: All tests pass.

- [ ] **Step 8: Commit**

```bash
git add src/app.rs
git commit -m "feat: wire audio output lifecycle into App

Background decode thread spawned on pending_file, polled each frame.
AudioOutput created on decode completion with fallible cpal init.
SharedAudioPlayback sent to sim thread via SetAudioShared."
```

---

### Task 7: Update sidebar audio controls — seek, volume, position display

**Files:**

- Modify: `src/ui/scope_panel.rs`

- [ ] **Step 1: Rewrite audio_controls function**

Replace the `audio_controls` function with:

```rust
fn audio_controls(ui: &mut egui::Ui, audio: &mut AudioUiState) {
    if ui.button("Open File...").clicked()
        && let Some(path) = rfd::FileDialog::new()
            .add_filter("Audio", &["wav", "flac", "ogg", "mp3"])
            .pick_file()
    {
        audio.pending_file = Some(path);
    }

    // Loading indicator
    if audio.decode_receiver.is_some() {
        ui.spinner();
        ui.label("Loading...");
        return;
    }

    if let Some(err) = &audio.load_error {
        ui.colored_label(egui::Color32::RED, err);
    }

    if let Some(path) = &audio.file_path
        && let Some(name) = path.file_name()
    {
        ui.label(name.to_string_lossy().as_ref());
    }

    let Some(shared) = &audio.shared else {
        return;
    };

    ui.separator();

    // Play/Pause + Loop
    ui.horizontal(|ui| {
        let playing = shared.playing.load(std::sync::atomic::Ordering::Relaxed);
        let play_label = if playing { "Pause" } else { "Play" };
        if ui.button(play_label).clicked() {
            shared
                .playing
                .store(!playing, std::sync::atomic::Ordering::Relaxed);
        }
        let mut looping = shared.looping.load(std::sync::atomic::Ordering::Relaxed);
        if ui.checkbox(&mut looping, "Loop").changed() {
            shared
                .looping
                .store(looping, std::sync::atomic::Ordering::Relaxed);
        }
    });

    // Seek bar
    let duration = shared.duration_secs();
    let mut pos_secs = shared.position_secs();
    let seek_bar = egui::Slider::new(&mut pos_secs, 0.0..=duration)
        .show_value(false)
        .trailing_fill(true);
    if ui.add(seek_bar).changed() {
        shared.seek_to_secs(pos_secs);
    }

    // Position / Duration label
    let pos_display = format_time(pos_secs);
    let dur_display = format_time(duration);
    ui.label(format!("{pos_display} / {dur_display}"));

    // Speed
    let mut speed = shared.speed.load(std::sync::atomic::Ordering::Relaxed);
    if ui
        .add(
            egui::Slider::new(&mut speed, 0.25..=4.0)
                .logarithmic(true)
                .text("Speed"),
        )
        .changed()
    {
        shared
            .speed
            .store(speed, std::sync::atomic::Ordering::Relaxed);
    }

    // Volume
    let mut volume = shared.volume.load(std::sync::atomic::Ordering::Relaxed);
    if ui
        .add(egui::Slider::new(&mut volume, 0.0..=1.0).text("Volume"))
        .changed()
    {
        shared
            .volume
            .store(volume, std::sync::atomic::Ordering::Relaxed);
    }
}

fn format_time(secs: f32) -> String {
    let total = secs as u32;
    let m = total / 60;
    let s = total % 60;
    format!("{m}:{s:02}")
}
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check`
Expected: Compiles.

- [ ] **Step 3: Manual test**

Run: `cargo run --release`
Expected: Load an audio file — should hear audio and see controls (play/pause, seek bar, speed, volume, position/duration display).

- [ ] **Step 4: Commit**

```bash
git add src/ui/scope_panel.rs
git commit -m "feat: enhanced audio controls with seek, volume, position display

Seek bar with position/duration labels, volume slider, loading
indicator. All controls read/write SharedAudioPlayback atomics."
```

---

### Task 8: Floating media overlay

**Files:**

- Create: `src/ui/media_overlay.rs`
- Modify: `src/ui/mod.rs`

- [ ] **Step 1: Create MediaOverlay struct**

Create `src/ui/media_overlay.rs`:

```rust
use std::sync::Arc;

use crate::audio_output::SharedAudioPlayback;
use crate::types::InputMode;

pub struct MediaOverlay {
    opacity: f32,
    fade_target: f32,
}

impl Default for MediaOverlay {
    fn default() -> Self {
        Self {
            opacity: 0.0,
            fade_target: 0.0,
        }
    }
}

impl MediaOverlay {
    /// Show the media overlay if appropriate. Call during the egui frame.
    ///
    /// `viewport_rect`: the screen area where the overlay should appear
    /// (full window in detached mode, right-of-sidebar in combined mode).
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        viewport_rect: egui::Rect,
        input_mode: InputMode,
        shared: Option<&Arc<SharedAudioPlayback>>,
    ) {
        // Only show for audio mode with loaded file
        let Some(shared) = shared else {
            self.opacity = 0.0;
            return;
        };
        if input_mode != InputMode::Audio {
            self.opacity = 0.0;
            return;
        }

        // Check if mouse is over viewport or overlay area
        let overlay_height = 44.0;
        let overlay_width = viewport_rect.width() * 0.8;
        let overlay_rect = egui::Rect::from_center_size(
            egui::pos2(
                viewport_rect.center().x,
                viewport_rect.bottom() - overlay_height / 2.0 - 12.0,
            ),
            egui::vec2(overlay_width, overlay_height),
        );

        let pointer_pos = ctx.input(|i| i.pointer.hover_pos());
        let hovering = pointer_pos.is_some_and(|p| {
            viewport_rect.contains(p) || overlay_rect.contains(p)
        });

        // Animate opacity
        self.fade_target = if hovering { 1.0 } else { 0.0 };
        let dt = ctx.input(|i| i.predicted_dt);
        let fade_speed = 1.0 / 0.2; // 200ms fade
        if self.opacity < self.fade_target {
            self.opacity = (self.opacity + dt * fade_speed).min(self.fade_target);
        } else {
            self.opacity = (self.opacity - dt * fade_speed).max(self.fade_target);
        }

        // Request repaint during animation
        if (self.opacity - self.fade_target).abs() > 0.01 {
            ctx.request_repaint();
        }

        if self.opacity < 0.01 {
            return;
        }

        let alpha = (self.opacity * 255.0) as u8;

        egui::Area::new(egui::Id::new("media_overlay"))
            .fixed_pos(overlay_rect.left_top())
            .show(ctx, |ui| {
                let frame = egui::Frame::new()
                    .fill(egui::Color32::from_black_alpha((180.0 * self.opacity) as u8))
                    .corner_radius(8.0)
                    .inner_margin(8.0);

                frame.show(ui, |ui| {
                    ui.set_min_width(overlay_width - 16.0);
                    ui.set_max_width(overlay_width - 16.0);

                    // Override text color for visibility on dark background
                    ui.visuals_mut().override_text_color =
                        Some(egui::Color32::from_white_alpha(alpha));

                    ui.horizontal(|ui| {
                        // Play/Pause
                        let playing = shared
                            .playing
                            .load(std::sync::atomic::Ordering::Relaxed);
                        let label = if playing { "\u{23F8}" } else { "\u{25B6}" };
                        if ui.button(label).clicked() {
                            shared.playing.store(
                                !playing,
                                std::sync::atomic::Ordering::Relaxed,
                            );
                        }

                        // Seek bar
                        let duration = shared.duration_secs();
                        let mut pos = shared.position_secs();
                        let slider = egui::Slider::new(&mut pos, 0.0..=duration)
                            .show_value(false)
                            .trailing_fill(true);
                        if ui.add(slider).changed() {
                            shared.seek_to_secs(pos);
                        }

                        // Time label
                        let pos_str = super::scope_panel::format_time(pos);
                        let dur_str = super::scope_panel::format_time(duration);
                        ui.label(format!("{pos_str} / {dur_str}"));
                    });
                });
            });
    }
}
```

- [ ] **Step 2: Register module and add to UiState**

In `src/ui/mod.rs`, add `pub mod media_overlay;` at the top.

Add the overlay to `UiState`:

```rust
pub media_overlay: media_overlay::MediaOverlay,
```

Initialize in `UiState::new()`:

```rust
media_overlay: media_overlay::MediaOverlay::default(),
```

Make `format_time` in `scope_panel.rs` public: `pub fn format_time(...)`.

- [ ] **Step 3: Call overlay from UiState::run()**

In `UiState::run()`, after the side panel block (after the `if self.panel_visible { ... } else { ... }` block, still inside the `ctx.run` closure), add:

```rust
// Media overlay — viewport is the area not covered by the panel
let viewport = egui::Rect::from_min_size(
    egui::pos2(self.panel_width, 0.0),
    egui::vec2(
        egui_ctx.screen_rect().width() - self.panel_width,
        egui_ctx.screen_rect().height(),
    ),
);
self.media_overlay.show(
    egui_ctx,
    viewport,
    self.input_mode,
    self.audio_ui.shared.as_ref(),
);
```

- [ ] **Step 4: Enable overlay in detached mode**

Two changes are needed to make the overlay work in detached mode:

**4a.** In `app.rs` `toggle_detach()`, set `panel_visible = false` when entering detached mode so the sidebar doesn't render on the viewport window:

```rust
// In the WindowMode::Combined arm, after creating the controls window:
if let Some(ui) = &mut self.ui {
    ui.panel_visible = false;
}
```

**4b.** In `app.rs` `handle_viewport_event()` `RedrawRequested`, always run the egui pass (not just in combined mode). Replace the conditional `egui_output` block (lines 190-200):

```rust
// Run egui frame — combined: panel + overlay, detached: overlay only
// (panel_visible is false in detached mode, so only overlay renders)
let timings = gpu.profiler.as_ref().map(|p| &p.history);
let egui_output = Some(ui.run(
    window,
    timings,
    self.sim_stats.as_ref(),
    Some(&sim_frame_info),
));
```

Also in `handle_viewport_event`, remove the combined-mode guard on event forwarding to egui (lines 122-130) — egui needs to receive events in detached mode too for the overlay to respond to hover/clicks:

```rust
// Always pass events to egui (overlay needs them in detached mode)
if let Some(ui) = &mut self.ui
    && let Some(window) = &self.window
{
    let response = ui.on_event(window, &event);
    if response.consumed {
        return;
    }
}
```

- [ ] **Step 5: Verify compilation**

Run: `cargo check`
Expected: Compiles.

- [ ] **Step 6: Manual test**

Run: `cargo run --release`
Expected: Load audio file. Hover over viewport — floating overlay fades in with play/pause, seek bar, and time display. Move mouse away — fades out. Test in both combined and detached modes.

- [ ] **Step 7: Commit**

```bash
git add src/ui/media_overlay.rs src/ui/mod.rs src/ui/scope_panel.rs src/app.rs
git commit -m "feat: floating media overlay on CRT viewport

Semi-transparent overlay with play/pause, seek bar, and time display.
Fades in/out on mouse hover. Works in both combined and detached modes."
```

---

### Task 9: Final cleanup and integration testing

**Files:**

- Modify: `src/beam/mod.rs` (if AudioSource references remain)
- Possibly: `src/main.rs`, `Cargo.toml`

- [ ] **Step 1: Check for unused imports and dead code**

Run: `cargo check`
Expected: Compiles with no errors. Fix any warnings about unused imports.

- [ ] **Step 2: Run full test suite**

Run: `cargo test`
Expected: All tests pass.

- [ ] **Step 3: Manual integration test**

Run: `cargo run --release`

Test checklist:

1. Open an audio file (WAV, FLAC, MP3)
2. Audio plays through speakers synchronized with CRT visualization
3. Play/pause works (sidebar and overlay)
4. Seek bar works (drag to different position)
5. Speed slider works (audio pitch changes, visual stays in sync)
6. Volume slider works
7. Loop toggle works (audio restarts at end)
8. Position/duration display updates correctly
9. Floating overlay appears/disappears on hover
10. Detached mode: overlay appears on viewport hover
11. Loading a second file replaces the first
12. App starts and runs fine with no audio device (if testable)

- [ ] **Step 4: Commit any final fixes**

```bash
git add -A
git commit -m "chore: final cleanup for audio playback feature"
```

- [ ] **Step 5: Clean up plan file**

Delete `docs/superpowers/plans/2026-03-20-audio-playback.md` and `docs/superpowers/specs/2026-03-20-audio-playback-design.md` — they are implementation artifacts, not permanent documentation.
