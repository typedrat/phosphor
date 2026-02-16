# Framerate-Independent Simulation Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Decouple sample generation from rendering by running a fixed-rate simulation thread that pushes samples through an rtrb ring buffer, making visual output framerate-independent and enabling future real-time audio input.

**Architecture:** Dedicated simulation thread owns `InputState` and generates samples in adaptive batches. It targets ~1ms wall-clock intervals, computing `batch_size = sample_rate * batch_interval` samples per wake. If a batch takes longer than the target interval, the thread increases the batch interval to amortize overhead (supporting MHz sample rates for raster mode). Finished `BeamSample`s (after arc-length resampling, aspect correction, energy scaling) are bulk-pushed into an rtrb SPSC ring buffer. The render thread (main/winit) drains all pending samples each frame, dispatches one beam write, and applies decay using simulation time (`sample_count / sample_rate`) instead of wall-clock dt. UI parameter changes flow from render → sim thread via a `crossbeam-channel`.

**Tech Stack:** `rtrb` 0.3 (SPSC ring buffer with bulk `read_chunk`/`write_chunk`), `crossbeam-channel` (command channel), `spin_sleep` (high-resolution thread pacing)

---

### Task 1: Add Dependencies and Migrate to tracing

**Files:**

- Modify: `Cargo.toml`
- Modify: `src/main.rs`
- Modify: `src/gpu/mod.rs`

**Step 1: Update dependencies in Cargo.toml**

Add new dependencies:

```toml
crossbeam-channel = "0.5"
spin_sleep = "1"
atomic_float = "1"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
tracing-appender = "0.2"
```

Remove old logging dependencies:

```toml
# Remove these:
log = "0.4"
env_logger = "0.11"
```

`rtrb` is already present (version 0.3.2).

**Step 2: Replace env_logger init with tracing-subscriber**

In `src/main.rs`, replace:

```rust
env_logger::init();
```

with:

```rust
let (non_blocking, _guard) = tracing_appender::non_blocking(std::io::stderr());
tracing_subscriber::fmt()
    .with_env_filter(
        tracing_subscriber::EnvFilter::from_default_env()
    )
    .with_writer(non_blocking)
    .init();
```

`tracing-appender::non_blocking` delegates all I/O to a background writer thread, so `tracing::info!` etc. never block the sim or render threads. The `_guard` must be held for the lifetime of `main()` to ensure buffered logs are flushed on exit. `RUST_LOG` works the same as before.

**Step 3: Replace all `log::` calls with `tracing::`**

Find all `log::info!`, `log::debug!`, `log::warn!`, `log::error!` calls in the codebase and replace with `tracing::info!`, `tracing::debug!`, `tracing::warn!`, `tracing::error!`. The macro syntax is identical for simple messages. For structured fields, `tracing` uses `tracing::info!(field = value, "message")` syntax — convert where appropriate (e.g. in `gpu/mod.rs` adapter info logging).

**Step 4: Verify it compiles and runs**

Run: `RUST_LOG=debug cargo run`
Expected: compiles, runs, log output appears in stderr as before

**Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock src/
git commit -m "chore: migrate from log+env_logger to tracing+tracing-subscriber

tracing provides structured, thread-aware logging with spans.
Essential for diagnosing the simulation thread. RUST_LOG env
var works the same as before."
```

---

### Task 2: Define SimCommand and Simulation Module

**Files:**

- Create: `src/simulation.rs`
- Modify: `src/main.rs` (add `mod simulation;`)

**Step 1: Create the simulation module with SimCommand enum**

Create `src/simulation.rs` with the `SimCommand` enum and re-exports. This is the public API for the simulation thread — the render thread sends commands, the sim thread processes them.

```rust
use std::path::PathBuf;

use crate::app::{InputMode, OscilloscopeState};
use crate::types::Resolution;

/// Commands sent from the render/UI thread to the simulation thread.
pub enum SimCommand {
    SetInputMode(InputMode),
    SetOscilloscopeParams(OscilloscopeState),
    SetFocus(f32),
    /// Viewport dimensions and offset for aspect ratio correction.
    /// `x_offset` is the sidebar width in pixels (0 when hidden or detached).
    SetViewport {
        width: f32,
        height: f32,
        x_offset: f32,
    },
    SetAccumResolution(Resolution),
    LoadAudioFile(PathBuf),
    SetAudioPlaying(bool),
    SetAudioLooping(bool),
    SetAudioSpeed(f32),
    LoadVectorFile(PathBuf),
    /// Sample rate change — carries the new producer from a resized channel.
    /// The render thread creates the new channel and swaps its consumer.
    SetSampleRate { rate: f32, producer: SampleProducer },
    Shutdown,
}
```

`OscilloscopeState` needs `Clone` derived (it currently has no derives). Add `#[derive(Clone)]` to `OscilloscopeState` in `src/app.rs`.

**Step 2: Add `mod simulation;` to `src/main.rs`**

Add `mod simulation;` alongside the other module declarations at the top of `src/main.rs`.

**Step 3: Verify it compiles**

Run: `cargo check`
Expected: compiles (SimCommand is defined but unused — that's fine)

**Step 4: Commit**

```bash
git add src/simulation.rs src/main.rs src/app.rs
git commit -m "feat: define SimCommand enum for simulation thread

Establishes the command channel API between the render thread and
the simulation thread. SimCommand variants cover all UI-driven
parameter changes that the sim thread needs to know about."
```

---

### Task 3: Extend SampleProducer/SampleConsumer with Bulk Operations

**Files:**

- Modify: `src/beam/mod.rs`

**Step 1: Write failing tests for bulk push and bulk drain**

Add tests to the existing `#[cfg(test)] mod tests` block in `src/beam/mod.rs`:

```rust
#[test]
fn bulk_push_and_drain() {
    let (mut tx, mut rx) = sample_channel(128);
    let samples: Vec<BeamSample> = (0..50)
        .map(|i| BeamSample {
            x: i as f32 * 0.01,
            y: 0.5,
            intensity: 1.0,
            dt: 0.001,
        })
        .collect();

    let pushed = tx.push_bulk(&samples);
    assert_eq!(pushed, 50);

    let drained = rx.drain();
    assert_eq!(drained.len(), 50);
    assert!((drained[0].x - 0.0).abs() < f32::EPSILON);
    assert!((drained[49].x - 0.49).abs() < f32::EPSILON);
}

#[test]
fn bulk_push_partial_when_full() {
    let (mut tx, mut rx) = sample_channel(4);
    let samples: Vec<BeamSample> = (0..10)
        .map(|_| BeamSample {
            x: 0.5,
            y: 0.5,
            intensity: 1.0,
            dt: 0.001,
        })
        .collect();

    let pushed = tx.push_bulk(&samples);
    assert_eq!(pushed, 4); // only 4 slots available

    assert_eq!(rx.drain().len(), 4);
}

#[test]
fn drain_up_to_respects_cap() {
    let (mut tx, mut rx) = sample_channel(128);
    let samples: Vec<BeamSample> = (0..100)
        .map(|i| BeamSample {
            x: i as f32 * 0.01,
            y: 0.5,
            intensity: 1.0,
            dt: 0.001,
        })
        .collect();

    tx.push_bulk(&samples);

    // Drain only 30 — 70 should remain
    let first = rx.drain_up_to(30);
    assert_eq!(first.len(), 30);

    // Drain the rest
    let second = rx.drain_up_to(1000);
    assert_eq!(second.len(), 70);

    // Nothing left
    let third = rx.drain_up_to(10);
    assert_eq!(third.len(), 0);
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p phosphor beam::tests::bulk_push beam::tests::drain_up_to`
Expected: FAIL — `push_bulk` and `drain_up_to` methods do not exist

**Step 3: Implement `push_bulk` on SampleProducer and update `drain` to use `read_chunk`**

Replace the `SampleProducer` and `SampleConsumer` impls in `src/beam/mod.rs`:

```rust
impl SampleProducer {
    /// Push a single sample. Returns `true` if successful, `false` if full.
    pub fn push(&mut self, sample: BeamSample) -> bool {
        self.inner.push(sample).is_ok()
    }

    /// Bulk-push samples using zero-copy write_chunk. Returns the number
    /// of samples actually written (may be less than `samples.len()` if
    /// the buffer doesn't have enough free slots).
    pub fn push_bulk(&mut self, samples: &[BeamSample]) -> usize {
        let available = self.inner.slots();
        let n = samples.len().min(available);
        if n == 0 {
            return 0;
        }
        if let Ok(chunk) = self.inner.write_chunk(n) {
            let (first, second) = chunk.as_mut_slices();
            let first_len = first.len();
            first.copy_from_slice(&samples[..first_len]);
            if !second.is_empty() {
                second.copy_from_slice(&samples[first_len..n]);
            }
            chunk.commit_all();
            n
        } else {
            0
        }
    }
}

impl SampleConsumer {
    /// Drain all pending samples using zero-copy read_chunk.
    pub fn drain(&mut self) -> Vec<BeamSample> {
        self.drain_up_to(usize::MAX)
    }

    /// Drain up to `max` pending samples using zero-copy read_chunk.
    /// Any samples beyond `max` remain in the buffer for the next call.
    pub fn drain_up_to(&mut self, max: usize) -> Vec<BeamSample> {
        let available = self.inner.slots();
        let count = available.min(max);
        if count == 0 {
            return Vec::new();
        }
        let chunk = self.inner.read_chunk(count).unwrap();
        let (first, second) = chunk.as_slices();
        let mut samples = Vec::with_capacity(count);
        samples.extend_from_slice(first);
        samples.extend_from_slice(second);
        chunk.commit_all();
        samples
    }
}
```

**Step 4: Run tests to verify they pass**

Run: `cargo test -p phosphor beam::tests`
Expected: all beam tests PASS

**Step 5: Commit**

```bash
git add src/beam/mod.rs
git commit -m "feat: add bulk push/drain to sample channel using rtrb chunks

SampleProducer::push_bulk uses write_chunk for zero-copy bulk writes.
SampleConsumer::drain uses read_chunk + as_slices for zero-copy bulk
reads. Critical for high sample counts (192kHz+, future raster mode)."
```

---

### Task 4: Build the Simulation Thread Loop

**Files:**

- Modify: `src/simulation.rs`

**Step 1: Implement the simulation thread function**

Expand `src/simulation.rs` to include the thread loop. The sim thread owns `InputState`, receives commands via `crossbeam_channel::Receiver<SimCommand>`, and pushes samples into `SampleProducer`.

The thread uses **adaptive batch intervals**: it targets ~1ms wall-clock intervals, computing `batch_size = sample_rate * batch_interval` samples per wake. If a batch consistently takes longer than the target, it increases the batch interval (doubling up to 10ms cap) to amortize overhead. If the thread is comfortably keeping up, it shrinks back toward 1ms. This supports sample rates from 44.1kHz (44 samples/batch) to MHz raster rates (thousands of samples/batch).

```rust
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::Receiver;

use crate::app::{InputMode, InputState, OscilloscopeState};
use crate::beam::SampleProducer;
use crate::types::Resolution;

const BEAM_ENERGY_SCALE: f32 = 5000.0;

/// Target batch interval bounds.
const MIN_BATCH_INTERVAL: Duration = Duration::from_millis(1);
const MAX_BATCH_INTERVAL: Duration = Duration::from_millis(10);

pub enum SimCommand {
    SetInputMode(InputMode),
    SetOscilloscopeParams(OscilloscopeState),
    SetFocus(f32),
    SetViewport {
        width: f32,
        height: f32,
        x_offset: f32,
    },
    SetAccumResolution(Resolution),
    LoadAudioFile(PathBuf),
    SetAudioPlaying(bool),
    SetAudioLooping(bool),
    SetAudioSpeed(f32),
    LoadVectorFile(PathBuf),
    /// Sample rate change — carries the new producer from a resized channel.
    /// The render thread creates the new channel and swaps its consumer.
    SetSampleRate { rate: f32, producer: SampleProducer },
    Shutdown,
}

/// State tracked by the simulation thread, derived from SimCommands.
struct SimState {
    input: InputState,
    focus: f32,
    viewport_width: f32,
    viewport_height: f32,
    accum_resolution: Resolution,
    sample_rate: f32,
}

impl SimState {
    fn new() -> Self {
        let input = InputState::default();
        let sample_rate = input.oscilloscope.sample_rate;
        Self {
            input,
            focus: 1.5,
            viewport_width: 800.0,
            viewport_height: 600.0,
            accum_resolution: Resolution::new(800, 600),
            sample_rate,
        }
    }

    fn aspect(&self) -> f32 {
        self.viewport_width / self.viewport_height.max(1.0)
    }

    fn apply_command(&mut self, cmd: SimCommand) {
        match cmd {
            SimCommand::SetInputMode(mode) => self.input.mode = mode,
            SimCommand::SetOscilloscopeParams(params) => {
                self.input.oscilloscope = params;
            }
            SimCommand::SetFocus(f) => self.focus = f,
            SimCommand::SetViewport { width, height, .. } => {
                self.viewport_width = width;
                self.viewport_height = height;
            }
            SimCommand::SetAccumResolution(res) => self.accum_resolution = res,
            SimCommand::LoadAudioFile(path) => self.input.load_audio_file(path),
            SimCommand::SetAudioPlaying(p) => self.input.audio.playing = p,
            SimCommand::SetAudioLooping(l) => self.input.audio.looping = l,
            SimCommand::SetAudioSpeed(s) => self.input.audio.speed = s,
            SimCommand::LoadVectorFile(path) => self.input.load_vector_file(path),
            SimCommand::SetSampleRate { rate, .. } => self.sample_rate = rate,
            SimCommand::Shutdown => {} // handled by caller
        }
    }
}

/// Run the simulation loop on the current thread. Blocks until Shutdown
/// is received or the command channel is disconnected.
pub fn run_simulation(
    mut producer: SampleProducer,
    commands: Receiver<SimCommand>,
) {
    let mut state = SimState::new();
    let mut batch_interval = MIN_BATCH_INTERVAL;
    let mut next_tick = Instant::now();

    loop {
        // Process all pending commands
        while let Ok(cmd) = commands.try_recv() {
            if matches!(cmd, SimCommand::Shutdown) {
                return;
            }
            // SetSampleRate carries a new producer — swap it before
            // apply_command, which only updates the rate field.
            if let SimCommand::SetSampleRate { rate, producer: new_prod } = cmd {
                producer = new_prod;
                state.sample_rate = rate;
                continue;
            }
            state.apply_command(cmd);
        }

        // Compute batch size from current sample rate and batch interval
        let batch_size = (state.sample_rate as f64
            * batch_interval.as_secs_f64()) as usize;
        let batch_size = batch_size.max(1);

        let gen_start = Instant::now();

        // Generate a batch of samples
        let samples = state.input.generate_samples_fixed(
            state.focus,
            state.aspect(),
            state.accum_resolution,
            state.sample_rate,
            batch_size,
        );

        // Push into ring buffer (partial write if buffer is near-full)
        if !samples.is_empty() {
            producer.push_bulk(&samples);
        }

        let gen_elapsed = gen_start.elapsed();

        // Adaptive batch interval:
        // If generation took >80% of the batch interval, double it (up to cap).
        // If generation took <20% of the batch interval, halve it (down to floor).
        if gen_elapsed > batch_interval.mul_f32(0.8) {
            batch_interval = (batch_interval * 2).min(MAX_BATCH_INTERVAL);
        } else if gen_elapsed < batch_interval.mul_f32(0.2) {
            batch_interval = (batch_interval / 2).max(MIN_BATCH_INTERVAL);
        }

        // Pace to target interval
        next_tick += batch_interval;
        let now = Instant::now();
        if next_tick > now {
            spin_sleep::sleep(next_tick - now);
        } else {
            // Fell behind — reset to avoid burst catch-up
            next_tick = now;
        }
    }
}

/// Spawn the simulation thread. Returns a join handle and command sender.
pub fn spawn_simulation(
    producer: SampleProducer,
) -> (thread::JoinHandle<()>, crossbeam_channel::Sender<SimCommand>) {
    let (tx, rx) = crossbeam_channel::unbounded();
    let handle = thread::Builder::new()
        .name("phosphor-sim".into())
        .spawn(move || {
            run_simulation(producer, rx);
        })
        .expect("failed to spawn simulation thread");
    (handle, tx)
}
```

**Step 2: Add `generate_samples_fixed` to InputState**

Add a new method to `InputState` in `src/app.rs` that generates a fixed number of samples at a constant dt (no wall-clock measurement). This replaces the existing `generate_samples` for the sim thread.

```rust
/// Generate a fixed number of samples at the given sample rate.
/// Unlike `generate_samples`, this does NOT measure wall-clock time —
/// dt is always `1/sample_rate`, making output deterministic.
pub fn generate_samples_fixed(
    &mut self,
    focus: f32,
    aspect: f32,
    accum_resolution: Resolution,
    sample_rate: f32,
    count: usize,
) -> Vec<BeamSample> {
    let spot_radius = focus / accum_resolution.width.max(1) as f32;
    let beam = BeamState { spot_radius };

    let mut samples = match self.mode {
        InputMode::Oscilloscope => {
            self.sync_oscilloscope_params();
            self.osc_source.sample_rate = sample_rate;
            if count == 0 {
                return Vec::new();
            }
            self.osc_source.generate(count, &beam)
        }
        InputMode::Audio => {
            let audio = &mut self.audio;
            if !audio.playing {
                return Vec::new();
            }
            let Some(source) = &mut audio.source else {
                return Vec::new();
            };
            let adj_count = (count as f32 * audio.speed) as usize;
            if adj_count == 0 {
                return Vec::new();
            }
            let samples = source.generate(adj_count, &beam);
            if source.is_finished() {
                if audio.looping {
                    source.seek(0.0);
                } else {
                    audio.playing = false;
                }
            }
            samples
        }
        InputMode::Vector => {
            if self.vector.segments.is_empty() {
                return Vec::new();
            }
            let mut src = crate::beam::vector::VectorSource {
                segments: self.vector.segments.clone(),
                beam_speed: self.vector.beam_speed,
                settling_time: self.vector.settling_time,
            };
            src.generate(0, &beam)
        }
        InputMode::External => Vec::new(),
    };

    // Aspect ratio correction
    if aspect > 1.0 {
        for s in &mut samples {
            s.x = 0.5 + (s.x - 0.5) / aspect;
        }
    } else if aspect < 1.0 {
        for s in &mut samples {
            s.y = 0.5 + (s.y - 0.5) * aspect;
        }
    }

    // Arc-length resample
    let mut samples =
        crate::beam::resample::arc_length_resample(&samples, spot_radius * 0.5);

    // Scale beam energy
    for s in &mut samples {
        s.intensity *= BEAM_ENERGY_SCALE;
    }

    samples
}
```

Move `BEAM_ENERGY_SCALE` from `src/app.rs` to `src/simulation.rs` and make the one in `app.rs` reference it, or just keep both (the old `generate_samples` is being phased out). For now, keep the constant in both files since `generate_samples` still exists until Task 6 removes it.

**Step 3: Verify it compiles**

Run: `cargo check`
Expected: compiles (warnings about unused `spawn_simulation` are fine)

**Step 4: Commit**

```bash
git add src/simulation.rs src/app.rs
git commit -m "feat: implement simulation thread loop with adaptive batch pacing

SimState owns InputState, processes SimCommands, and generates samples
in adaptive batches. Targets ~1ms batch intervals, scaling up to 10ms
under load (for MHz raster rates). Samples are bulk-pushed into the
rtrb ring buffer. generate_samples_fixed produces deterministic output
without wall-clock time measurement."
```

---

### Task 5: Wire Simulation Thread into App

**Files:**

- Modify: `src/main.rs`

This is the main integration task. The `App` struct gains a `SampleConsumer`, a `Sender<SimCommand>`, and a `JoinHandle`. On startup (`resumed`), the sim thread is spawned. On `RedrawRequested`, samples are drained from the consumer instead of generated inline. UI parameter changes send `SimCommand`s.

**Step 1: Add sim thread fields to App**

Add to the `App` struct:

```rust
struct App {
    // ... existing fields ...
    sim_consumer: Option<SampleConsumer>,
    sim_commands: Option<crossbeam_channel::Sender<SimCommand>>,
    sim_handle: Option<std::thread::JoinHandle<()>>,
    sample_rate: f32,
}
```

Initialize all three as `None` in `Default`, with `sample_rate: 44100.0`.

**Step 2: Spawn sim thread in `resumed`**

After creating `GpuState` and `UiState`, create the sample channel and spawn the simulation thread:

```rust
// In resumed():
let (producer, consumer) = crate::beam::sample_channel(65536);
let (handle, cmd_tx) = crate::simulation::spawn_simulation(producer);

// Send initial viewport dimensions
let size = window.inner_size();
let _ = cmd_tx.send(SimCommand::SetViewport {
    width: size.width as f32,
    height: size.height as f32,
    x_offset: 0.0,
});

self.sim_consumer = Some(consumer);
self.sim_commands = Some(cmd_tx);
self.sim_handle = Some(handle);
```

**Step 3: Drain samples in RedrawRequested instead of generating inline**

In `handle_viewport_event` → `RedrawRequested`, replace:

```rust
let samples = ui.input.generate_samples(ui.focus, aspect, gpu.accum.resolution);
```

with:

```rust
// Frame budget cap: drain at most 2x the frame interval worth of samples
// to prevent catastrophic decay during render stalls. Adapts to the
// monitor's actual refresh rate (e.g. 33ms at 60Hz, 14ms at 144Hz).
let max_dt = self.frame_interval.as_secs_f32() * 2.0;
let max_samples = (self.sample_rate * max_dt) as usize;
let samples = self.sim_consumer.as_mut()
    .map(|c| c.drain_up_to(max_samples))
    .unwrap_or_default();
let sim_dt = if samples.is_empty() {
    0.0
} else {
    samples.len() as f32 / self.sample_rate
};
```

**Step 4: Use sim_dt for decay instead of wall-clock dt**

In `GpuState::render`, change the dt calculation. The method signature changes to accept an explicit `dt` parameter:

```rust
pub fn render(
    &mut self,
    samples: &[BeamSample],
    dt: f32,
    egui: Option<&EguiRenderOutput>,
) -> Result<(), wgpu::SurfaceError> {
    // Remove: let now = Instant::now();
    // Remove: let dt = now.duration_since(self.last_frame).as_secs_f32();
    // Remove: self.last_frame = now;

    // ... rest of render, using the passed-in dt for decay ...
    let decay_params = self.decay_params.with_dt(dt);
    // ...
}
```

Remove `last_frame: Instant` from `GpuState`.

Update the call site:

```rust
match gpu.render(&samples, sim_dt, egui_output.as_ref()) {
```

**Step 5: Send SimCommands when UI params change**

In `handle_viewport_event` → `RedrawRequested`, after running the egui frame, diff the UI state and send relevant SimCommands. The key parameters to forward:

```rust
if let Some(tx) = &self.sim_commands {
    // These are cheap to send every frame — the sim thread just overwrites fields
    let _ = tx.send(SimCommand::SetFocus(ui.focus));
    // Viewport: subtract sidebar width when panel is visible in combined mode
    let sidebar_width = if self.mode == WindowMode::Combined && ui.panel_visible {
        // egui SidePanel default_width is 220, but actual width may differ.
        // We'll get the actual width from the egui response in a later step.
        220.0
    } else {
        0.0
    };
    let _ = tx.send(SimCommand::SetViewport {
        width: gpu.surface_config.width as f32 - sidebar_width,
        height: gpu.surface_config.height as f32,
        x_offset: sidebar_width,
    });
}
```

Note: Getting the actual sidebar width from egui requires capturing the `SidePanel` response (its `inner_rect`). This will be refined in Task 7. For now, use the default width.

**Step 6: Send Shutdown on exit**

In `CloseRequested` and the exit paths, send `SimCommand::Shutdown` and join the thread:

```rust
WindowEvent::CloseRequested => {
    if let Some(tx) = self.sim_commands.take() {
        let _ = tx.send(SimCommand::Shutdown);
    }
    if let Some(handle) = self.sim_handle.take() {
        let _ = handle.join();
    }
    event_loop.exit();
}
```

**Step 7: Verify it compiles and runs**

Run: `cargo run`
Expected: Application launches, circle renders from oscilloscope mode. Visual output should look similar to before, though timing may feel slightly different.

**Step 8: Commit**

```bash
git add src/main.rs src/gpu/mod.rs
git commit -m "feat: wire simulation thread into render loop

Simulation thread spawned on startup, samples drained from rtrb ring
buffer each frame instead of generated inline. Decay uses simulation
time (sample_count / sample_rate) instead of wall-clock dt. GpuState
no longer tracks last_frame time. SimCommands sent for focus and
viewport changes."
```

---

### Task 6: Forward UI Parameter Changes as SimCommands

**Files:**

- Modify: `src/main.rs`
- Modify: `src/ui/mod.rs`
- Modify: `src/ui/scope_panel.rs`

The scope panel currently mutates `InputState` directly (it takes `&mut InputState`). Since `InputState` now lives on the sim thread, the UI needs to send `SimCommand`s instead. The approach: the UI still mutates local "shadow" copies of the parameters (for immediate visual feedback in sliders), and the render thread diffs and sends commands.

**Step 1: Remove `input: InputState` from `UiState`**

`UiState` no longer owns `InputState`. Instead, it keeps the UI-facing parameter copies that already exist: `OscilloscopeState`, `AudioState`, `VectorState`, `ExternalState`, and `InputMode`. These are already separate structs in `src/app.rs`.

Replace `pub input: InputState` in `UiState` with individual state fields:

```rust
pub struct UiState {
    // ... existing fields ...
    pub input_mode: InputMode,
    pub oscilloscope: OscilloscopeState,
    pub audio_ui: AudioUiState,  // UI-only audio state (no AudioSource)
    pub vector_ui: VectorUiState, // UI-only vector state (no segment data)
    pub external: ExternalState,
    // Remove: pub input: InputState,
}
```

Define minimal UI-only state structs for audio/vector (the actual `AudioSource` and segment data live on the sim thread):

```rust
pub struct AudioUiState {
    pub file_path: Option<PathBuf>,
    pub playing: bool,
    pub looping: bool,
    pub speed: f32,
    pub load_error: Option<String>,
}

pub struct VectorUiState {
    pub file_path: Option<PathBuf>,
    pub segment_count: usize,
    pub beam_speed: f32,
    pub settling_time: f32,
    pub looping: bool,
    pub load_error: Option<String>,
}
```

**Step 2: Update scope_panel to work with the new UI state**

Change the `scope_panel` function signature to take the individual state types instead of `&mut InputState`. The function logic stays mostly the same — it just references different struct fields.

**Step 3: Send SimCommands from the render thread**

In `handle_viewport_event`, after running the egui frame, check which parameters changed and send the corresponding `SimCommand`s. For file loads (audio/vector), the UI stores a `pending_file: Option<PathBuf>` that is consumed each frame:

```rust
// In RedrawRequested, after egui runs:
if let Some(tx) = &self.sim_commands {
    // Oscilloscope params
    let _ = tx.send(SimCommand::SetOscilloscopeParams(
        ui.oscilloscope.clone(),
    ));
    // Input mode
    let _ = tx.send(SimCommand::SetInputMode(ui.input_mode));
    // Audio controls
    let _ = tx.send(SimCommand::SetAudioPlaying(ui.audio_ui.playing));
    let _ = tx.send(SimCommand::SetAudioLooping(ui.audio_ui.looping));
    let _ = tx.send(SimCommand::SetAudioSpeed(ui.audio_ui.speed));
    // Pending file loads
    if let Some(path) = ui.audio_ui.pending_file.take() {
        let _ = tx.send(SimCommand::LoadAudioFile(path));
    }
    if let Some(path) = ui.vector_ui.pending_file.take() {
        let _ = tx.send(SimCommand::LoadVectorFile(path));
    }
}
```

**Step 4: Remove old generate_samples call path**

Delete the old `generate_samples` method from `InputState` (and `BEAM_ENERGY_SCALE` from `src/app.rs`) since all sample generation now goes through `generate_samples_fixed` on the sim thread. Keep `InputState` in `src/app.rs` since the sim thread still uses it, but it no longer needs `last_generate: Instant`.

Remove the `last_generate` field and wall-clock dt measurement from `InputState`.

**Step 5: Verify it compiles and runs**

Run: `cargo run`
Expected: UI controls work — changing waveform, frequency, etc. updates the display. Audio file loading works. Vector file loading works.

**Step 6: Commit**

```bash
git add src/main.rs src/ui/mod.rs src/ui/scope_panel.rs src/app.rs
git commit -m "feat: forward UI parameter changes as SimCommands

UI no longer directly mutates InputState. Scope panel works with
UI-local state copies, and the render thread sends SimCommands
for all parameter changes. File loads are sent as commands and
executed on the sim thread."
```

---

### Task 7: Fix Aspect Ratio to Account for Sidebar Width

**Files:**

- Modify: `src/ui/mod.rs`
- Modify: `src/main.rs`

**Step 1: Capture actual sidebar width from egui SidePanel response**

`egui::SidePanel::show` returns a response containing `inner_rect`. Capture the panel's occupied width:

```rust
// In UiState::run():
let mut panel_width = 0.0f32;
if self.panel_visible {
    let response = egui::SidePanel::left("control_panel")
        .default_width(220.0)
        .show(ctx, |ui| {
            // ... panel contents ...
        });
    panel_width = response.response.rect.width();
}
```

Add a `pub panel_width: f32` field to `UiState` (or return it from `run`).

**Step 2: Use actual sidebar width in viewport calculation**

In `handle_viewport_event` → `RedrawRequested`, replace the hardcoded 220.0:

```rust
let sidebar_width = if self.mode == WindowMode::Combined && ui.panel_visible {
    ui.panel_width
} else {
    0.0
};
let _ = tx.send(SimCommand::SetViewport {
    width: gpu.surface_config.width as f32 - sidebar_width,
    height: gpu.surface_config.height as f32,
    x_offset: sidebar_width,
});
```

**Step 3: Update composite pass to use viewport offset**

Add `viewport_offset` to `CompositeParams`:

```rust
pub struct CompositeParams {
    // ... existing fields ...
    pub viewport_offset: [f32; 2], // replaces _pad
}
```

Update the WGSL shader to offset the UV calculation:

```wgsl
// In fs_main:
let pixel = in.position.xy - vec2<f32>(params.viewport_offset.x, params.viewport_offset.y);
let uv = pixel / params.viewport_size;
```

This shifts the CRT render area to start at `x_offset` pixels from the left edge.

**Step 4: Set viewport_offset in RedrawRequested**

```rust
gpu.composite_params.viewport_offset = [sidebar_width, 0.0];
gpu.composite_params.viewport_size = [
    gpu.surface_config.width as f32 - sidebar_width,
    gpu.surface_config.height as f32,
];
```

**Step 5: Verify it compiles and runs**

Run: `cargo run`
Expected: The CRT circle remains a circle (correct aspect ratio) regardless of sidebar visibility. Toggling the sidebar (clicking X / hamburger) immediately updates the aspect ratio. The CRT viewport fills only the area to the right of the sidebar.

**Step 6: Commit**

```bash
git add src/ui/mod.rs src/main.rs src/gpu/composite.rs src/gpu/composite.wgsl
git commit -m "fix: aspect ratio accounts for sidebar width and viewport offset

Captures actual sidebar width from egui SidePanel response. Viewport
dimensions sent to sim thread exclude the sidebar area. Composite shader
offsets UV coordinates by viewport_offset so CRT output renders in the
correct sub-region, not behind the sidebar."
```

---

### Task 8: Simulation Observability — Logging and Engineer Panel Stats

**Files:**

- Create: `src/simulation_stats.rs`
- Modify: `src/simulation.rs`
- Modify: `src/ui/engineer_panel.rs`
- Modify: `src/main.rs`

**Step 1: Create SimStats shared struct**

Create `src/simulation_stats.rs` with a lock-free shared stats struct using atomics:

```rust
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use atomic_float::AtomicF32;

/// Statistics shared between the simulation thread (writer) and
/// the render/UI thread (reader). All fields use relaxed atomics —
/// individual reads may be slightly stale but that's fine for display.
pub struct SimStats {
    /// Current adaptive batch interval in seconds.
    pub batch_interval: AtomicF32,
    /// Samples generated per second (smoothed, updated ~once per second).
    pub throughput: AtomicF32,
    /// Cumulative count of samples dropped due to full ring buffer.
    pub samples_dropped: AtomicU32,
    /// Ring buffer capacity (updated on resize).
    pub buffer_capacity: AtomicU32,
}

impl SimStats {
    pub fn new(buffer_capacity: u32) -> Arc<Self> {
        Arc::new(Self {
            batch_interval: AtomicF32::new(0.001), // 1ms default
            throughput: AtomicF32::new(0.0),
            samples_dropped: AtomicU32::new(0),
            buffer_capacity: AtomicU32::new(buffer_capacity),
        })
    }
}
```

Add `mod simulation_stats;` to `src/main.rs`.

**Step 2: Wire SimStats into simulation thread**

Pass `Arc<SimStats>` to `run_simulation` / `spawn_simulation`. Update the sim loop:

```rust
// At end of each batch:
let dropped = batch_size - pushed;
if dropped > 0 {
    stats.samples_dropped.fetch_add(dropped as u32, Ordering::Relaxed);
    tracing::warn!(dropped, "samples dropped (ring buffer full)");
}

// Update batch interval stat
stats.batch_interval.store(batch_interval.as_secs_f32(), Ordering::Relaxed);

// Throughput: count samples over a 1-second window
samples_this_second += pushed;
if second_timer.elapsed() >= Duration::from_secs(1) {
    stats.throughput.store(samples_this_second as f32, Ordering::Relaxed);
    samples_this_second = 0;
    second_timer = Instant::now();
}
```

Add logging at key lifecycle points:

The sim thread enters a `tracing::info_span!("sim")` so all its output is automatically tagged with the thread context:

```rust
// At top of run_simulation():
let _span = tracing::info_span!("sim").entered();

tracing::info!(sample_rate = state.sample_rate, "thread started");
tracing::info!("thread shutting down");
tracing::info!(rate, capacity = cap, "sample rate changed, buffer resized");
tracing::debug!(interval_ms = batch_interval.as_secs_f32() * 1000.0, "batch interval adapted");
```

**Step 3: Display stats in engineer panel**

Add a "Simulation" section to the engineer panel. The render thread reads `SimStats` + computes its own per-frame stats:

```rust
// In engineer_panel(), add a new section:
ui.separator();
ui.heading("Simulation");

if let Some(stats) = sim_stats {
    let buffer_pending = consumer_slots; // passed from main.rs
    let buffer_cap = stats.buffer_capacity.load(Ordering::Relaxed);
    let fill_pct = if buffer_cap > 0 {
        100.0 * buffer_pending as f32 / buffer_cap as f32
    } else {
        0.0
    };

    ui.label(format!("Samples/frame: {samples_this_frame}"));
    ui.label(format!("sim_dt: {:.2} ms", sim_dt * 1000.0));
    ui.label(format!(
        "Throughput: {:.1} ksps",
        stats.throughput.load(Ordering::Relaxed) / 1000.0
    ));
    ui.label(format!(
        "Buffer: {buffer_pending}/{buffer_cap} ({fill_pct:.0}%)"
    ));
    ui.label(format!(
        "Batch interval: {:.1} ms",
        stats.batch_interval.load(Ordering::Relaxed) * 1000.0
    ));
    let dropped = stats.samples_dropped.load(Ordering::Relaxed);
    if dropped > 0 {
        ui.colored_label(
            egui::Color32::YELLOW,
            format!("Dropped: {dropped}"),
        );
    }
}
```

The `consumer_slots` (pending count) can be queried from `SampleConsumer` by adding a `pub fn pending(&self) -> usize` method that returns `self.inner.slots()`.

**Step 4: Verify it compiles and runs**

Run: `RUST_LOG=debug cargo run`
Expected: Sim thread lifecycle messages in stderr. Engineer panel shows simulation stats section with live-updating values.

**Step 5: Commit**

```bash
git add src/simulation_stats.rs src/simulation.rs src/ui/engineer_panel.rs src/main.rs src/beam/mod.rs
git commit -m "feat: add simulation thread observability

Shared SimStats struct (lock-free atomics) exposes batch interval,
throughput, buffer fill, and drop count. Engineer panel displays
live simulation statistics. Logging at info/debug/warn levels for
sim thread lifecycle, batch adaptation, and buffer overflows."
```

---

### Task 9: Clean Up and Final Verification

**Files:**

- Modify: `src/app.rs` (remove dead code)
- Modify: `src/main.rs` (remove unused imports)

**Step 1: Remove dead code**

- Remove `generate_samples` from `InputState` (replaced by `generate_samples_fixed`)
- Remove `last_generate: Instant` from `InputState`
- Remove `BEAM_ENERGY_SCALE` from `src/app.rs` (now in `src/simulation.rs`)
- Remove `last_frame: Instant` from `GpuState` (if not already done in Task 5)
- Clean up any unused imports

**Step 2: Run full test suite**

Run: `cargo test --workspace`
Expected: all tests pass

**Step 3: Run clippy**

Run: `cargo clippy --workspace -- -D warnings`
Expected: no warnings

**Step 4: Manual verification**

Run: `cargo run --release`
Verify:

- Oscilloscope circle is smooth and consistent
- Changing phosphor type works
- Audio playback works
- Toggling sidebar doesn't distort the display
- Detached mode (Ctrl+D) works
- Fullscreen (Ctrl+F) works
- No visible brightness jumps when resizing the window

**Step 5: Commit**

```bash
git add -A
git commit -m "refactor: clean up dead code from pre-simulation-thread architecture

Remove generate_samples, last_generate, wall-clock dt measurement,
and other code superseded by the fixed-rate simulation thread."
```
