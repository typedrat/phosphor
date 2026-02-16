# Framerate-Independent Simulation Design

## Problem

Sample generation and decay are coupled to the render frame rate. `InputState::generate_samples()` measures wall-clock dt, computes `sample_rate * dt` samples, and passes them to `GpuState::render()` which applies decay with that same dt. This means:

- Frame drops/hitches cause visible brightness jumps (more or fewer samples per frame)
- Variable VSync or compositor quirks cause uneven energy deposition
- Real-time audio input (future) can't block the render loop

## Goals

1. **Consistent visual output** regardless of frame rate — same input always produces same simulation result
2. **Architecture for real-time audio input** — dedicated thread that can block on audio device reads
3. **Fixed-rate decay** — decay ticks at the simulation sample rate, not wall-clock frame dt

## Architecture

### Thread Model

```
+---------------------+         rtrb SPSC           +----------------------+
|  Simulation Thread  |  ----- ring buffer ------>  |   Render Thread      |
|                     |    (BeamSample x 64K)       |   (main/winit)       |
|  Fixed-rate loop    |                             |                      |
|  at sample_rate     |  <-- crossbeam-channel ---  |  UI param changes    |
|  (e.g. 44100 Hz)    |       (SimCommand)          |  phosphor switches   |
|                     |                             |  mode changes        |
|  Generate samples   |                             |                      |
|  Arc-length resample|                             |  Drain ring buffer   |
|  Aspect correction  |                             |  One beam write      |
|  Energy scaling     |                             |  One decay (dt=N/sr) |
|  Push to ring buf   |                             |  Resolve+composite   |
+---------------------+                             +----------------------+
```

### Simulation Thread

Owns `InputState` (moved from main thread). Runs a tight loop at the configured sample rate.

Each tick:

1. `try_recv` pending `SimCommand`s and apply parameter changes
2. Generate one tick's worth of samples (dt = 1/sample_rate, always constant)
3. Arc-length resample, aspect correction, energy scaling
4. Bulk-write into `rtrb` ring buffer via `write_chunk`

Pacing: `spin_sleep` to maintain fixed tick rate. If the thread falls behind, reset the tick clock (don't burst catch-up).

Backpressure: if the ring buffer is full, drop samples (push fails). This means the GPU has stalled for >1 second — an edge case.

### Render Thread (Main Thread)

Owns `GpuState`, `UiState`, winit event loop. On `RedrawRequested`:

1. Drain samples from ring buffer via `read_chunk`, up to a **frame budget cap** of `sample_rate * 2 * frame_interval` samples, derived from the monitor's actual refresh rate (e.g. 33ms at 60Hz, 14ms at 144Hz). Any excess stays in the buffer for the next frame.
2. Compute simulation dt: `sim_dt = drained_count / sample_rate`
3. One beam write dispatch with drained samples
4. One decay pass with `sim_dt` (mathematically equivalent to per-tick decay for exponential: `exp(-dt/tau)^N = exp(-N*dt/tau)`)
5. Spectral resolve, faceplate scatter, composite as before

The frame budget cap prevents catastrophic decay during render stalls — if the GPU hangs for 500ms, the backlog is consumed over multiple frames rather than applied as one massive `exp(-0.5/tau)` step. No wall-clock dt measurement for simulation. Frame pacing remains for vsync only.

### Command Channel

`crossbeam_channel::unbounded::<SimCommand>()` carries UI parameter changes to the sim thread:

```rust
enum SimCommand {
    SetInputMode(InputMode),
    SetOscilloscopeParams(OscilloscopeState),
    SetFocus(f32),
    SetViewport { width: f32, height: f32, x_offset: f32 },
    SetAccumResolution(Resolution),
    LoadAudioFile(PathBuf),
    SetAudioPlaying(bool),
    SetAudioLooping(bool),
    SetAudioSpeed(f32),
    LoadVectorFile(PathBuf),
    SetSampleRate { rate: f32, producer: SampleProducer },
    Shutdown,
}
```

Most commands are cheap field writes. `LoadAudioFile` does I/O on the sim thread (doesn't block rendering).

### Sample Buffer

Keep `rtrb` for the high-throughput sample path. It provides:

- Zero-copy `read_chunk`/`write_chunk` for bulk contiguous access
- SPSC optimization (exactly our topology)
- Purpose-built for real-time audio workloads

The existing `SampleProducer`/`SampleConsumer` wrappers are extended to use `write_chunk`/`read_chunk` for batched operations.

Buffer capacity scales with sample rate: `max(65536, sample_rate * 0.5)` (~500ms of buffer). When the sample rate changes, the render thread creates a new channel with the appropriate capacity, sends the new `SampleProducer` to the sim thread via `SimCommand::SetSampleRate { rate, producer }`, and swaps in the new `SampleConsumer`. The old channel is dropped.

Future raster operations will vastly increase sample counts — bulk reads and dynamic buffer sizing are critical.

### Aspect Ratio and Viewport Fix

Currently `aspect = window_width / window_height` uses the full window including the sidebar. Fix:

1. When sidebar is open (Combined mode): `viewport_width = window_width - sidebar_width`
2. Aspect ratio: `viewport_width / window_height`
3. Viewport offset: CRT output is positioned at `x_offset = sidebar_width`, not centered in the full window
4. The sim thread receives `SetViewport { width, height, x_offset }` when the window resizes or sidebar toggles
5. The composite pass uses the viewport offset to render into the correct sub-region

### Decay Timing

Decay uses simulation time, not wall clock:

- `sim_dt = drained_sample_count / sample_rate`
- Exponential (tier 2): `exp(-sim_dt / tau)` — mathematically identical to per-tick application
- Power-law (tier 3): elapsed time advances by `sim_dt`
- Instant (tier 1): cleared after spectral resolve, same as before
- When no samples are drained (sim thread paused or buffer empty): `sim_dt = 0`, no decay applied

### Observability

**Logging (stderr via `tracing` crate — replaces `log` + `env_logger`):**

- `info`: Sim thread start/stop, sample rate changes, buffer resize (with old/new capacity)
- `debug`: Batch interval adaptation changes
- `warn`: Buffer overflow (samples dropped because ring buffer full)

**Engineer panel statistics** (shared `Arc<SimStats>` with atomic fields, sim thread writes, render thread reads):

- Samples drained this frame / sim_dt
- Ring buffer fill level (samples pending / capacity)
- Current adaptive batch interval
- Cumulative samples dropped (overflow count)
- Actual sim throughput (samples/sec, smoothed)

`SimStats` uses `AtomicF32` (from `atomic_float`) for float fields and `AtomicU32` for counters. No locks, no channel — just atomic stores from sim thread, atomic loads from render thread.

### Dependencies

- **Keep:** `rtrb` (sample ring buffer with bulk read/write)
- **Add:** `atomic_float` (ergonomic atomic f32 without manual bit-casting)
- **Add:** `crossbeam-channel` (command channel, sim thread <-> render thread)
- **Add:** `spin_sleep` (high-resolution thread pacing for sim loop)
- **Replace:** `log` + `env_logger` → `tracing` + `tracing-subscriber` + `tracing-appender` (structured, thread-aware logging with spans; non-blocking writer so log calls never stall the sim or render threads)
