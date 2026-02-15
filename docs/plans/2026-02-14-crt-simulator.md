# CRT Simulator Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build a physically-based X-Y CRT simulator with spectral phosphor rendering, multiple input modes, and an egui control UI.

**Architecture:** Accumulation buffer on the GPU stores spectral energy per pixel. Three GPU passes per frame: beam write (compute), decay (compute), tonemap (fragment). All input modes produce a common `BeamSample` stream. egui renders as an overlay on top of the CRT output.

**Tech Stack:** Rust (edition 2024), wgpu 28, winit 0.31, egui 0.33, symphonia 0.5, glam 0.32

**Design docs:** `docs/design-overview.md`, `docs/design-phosphor-model.md`, `docs/design-beam-input.md`, `docs/design-gpu-pipeline.md`, `docs/design-ui.md`

---

### Task 1: Spectral Band Foundation

**Files:**

- Create: `src/phosphor/mod.rs`
- Create: `src/phosphor/spectral.rs`
- Modify: `src/main.rs` (add module declaration)

**Context:** The spectral band system is the foundation everything else builds on. `SPECTRAL_BANDS` is a compile-time constant; all buffer layouts, shader loops, and CIE integration weights derive from it. Read `docs/design-phosphor-model.md` and `docs/design-gpu-pipeline.md` (Spectral Band Layout section).

**Step 1: Write the failing tests**

In `src/phosphor/spectral.rs`, define the module and write tests:

```rust
/// Number of spectral bands across the visible spectrum.
/// Changing this value propagates through the entire pipeline.
pub const SPECTRAL_BANDS: usize = 16;

/// Visible spectrum range in nanometers.
pub const WAVELENGTH_MIN: f32 = 380.0;
pub const WAVELENGTH_MAX: f32 = 780.0;
pub const BAND_WIDTH: f32 = (WAVELENGTH_MAX - WAVELENGTH_MIN) / SPECTRAL_BANDS as f32;

/// Returns the (min, max) wavelength range in nm for a given band index.
pub fn band_range(band: usize) -> (f32, f32) {
    todo!()
}

/// Returns the center wavelength in nm for a given band index.
pub fn band_center(band: usize) -> f32 {
    todo!()
}

/// CIE 1931 color matching function values (x_bar, y_bar, z_bar),
/// pre-integrated over each spectral band. Used to convert spectral
/// energy to XYZ tristimulus values.
///
/// Returns a [SPECTRAL_BANDS] array of (x, y, z) tuples.
pub fn cie_integration_weights() -> [(f32, f32, f32); SPECTRAL_BANDS] {
    todo!()
}

/// Compute emission weights for a Gaussian emission peak.
/// Returns normalized weights (sum = 1.0) across SPECTRAL_BANDS.
///
/// `peak_nm`: center wavelength of emission in nm
/// `fwhm_nm`: full width at half maximum in nm
pub fn gaussian_emission_weights(peak_nm: f32, fwhm_nm: f32) -> [f32; SPECTRAL_BANDS] {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn band_ranges_cover_visible_spectrum() {
        let (min, _) = band_range(0);
        let (_, max) = band_range(SPECTRAL_BANDS - 1);
        assert!((min - WAVELENGTH_MIN).abs() < 0.01);
        assert!((max - WAVELENGTH_MAX).abs() < 0.01);
    }

    #[test]
    fn band_ranges_are_contiguous() {
        for i in 0..SPECTRAL_BANDS - 1 {
            let (_, max_i) = band_range(i);
            let (min_next, _) = band_range(i + 1);
            assert!((max_i - min_next).abs() < 0.01);
        }
    }

    #[test]
    fn band_centers_are_midpoints() {
        for i in 0..SPECTRAL_BANDS {
            let (min, max) = band_range(i);
            let center = band_center(i);
            assert!((center - (min + max) / 2.0).abs() < 0.01);
        }
    }

    #[test]
    fn gaussian_weights_are_normalized() {
        let weights = gaussian_emission_weights(520.0, 40.0);
        let sum: f32 = weights.iter().sum();
        assert!((sum - 1.0).abs() < 0.001, "sum was {sum}");
    }

    #[test]
    fn gaussian_weights_peak_at_correct_band() {
        // 520nm should peak around band 5-6
        let weights = gaussian_emission_weights(520.0, 40.0);
        let peak_band = weights
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .unwrap()
            .0;
        let (min, max) = band_range(peak_band);
        assert!(
            520.0 >= min && 520.0 < max,
            "peak band {peak_band} range [{min}, {max}] doesn't contain 520nm"
        );
    }

    #[test]
    fn cie_weights_are_nonnegative_luminance() {
        let weights = cie_integration_weights();
        for (x, y, z) in &weights {
            assert!(x.is_finite());
            assert!(y.is_finite());
            assert!(z.is_finite());
            // Y (luminance) should be non-negative
            assert!(*y >= 0.0);
        }
    }
}
```

**Step 2: Set up module structure**

`src/phosphor/mod.rs`:

```rust
pub mod spectral;
```

`src/main.rs`:

```rust
mod phosphor;

fn main() {
    println!("Hello, world!");
}
```

**Step 3: Run tests to verify they fail**

Run: `cargo test -- phosphor::spectral`
Expected: FAIL (todo! panics)

**Step 4: Implement the spectral functions**

Replace the `todo!()` bodies:

- `band_range` / `band_center`: straightforward arithmetic from constants.
- `gaussian_emission_weights`: compute `exp(-0.5 * ((center - peak) / sigma)^2)` per band and normalize. `sigma = fwhm / 2.355`.
- `cie_integration_weights`: embed the CIE 1931 2-degree observer data (tabulated at 5nm intervals, 380-780nm, ~81 rows from CIE 15:2004). Integrate over each band by averaging the data points within the band's wavelength range, multiply by band width.

The CIE data should be a `const` array in the source. It's a well-known, unchanging dataset.

**Step 5: Run tests to verify they pass**

Run: `cargo test -- phosphor::spectral`
Expected: PASS

**Step 6: Commit**

```bash
git add src/phosphor/ src/main.rs
git commit -m "feat: add spectral band system with CIE integration weights"
```

---

### Task 2: Phosphor Type Database

**Files:**

- Create: `src/phosphor/decay.rs`
- Modify: `src/phosphor/mod.rs` (add types and database)

**Context:** Define the `PhosphorLayer`, `PhosphorType` structs and build the database of common phosphors from the Tektronix reference data. Read `docs/design-phosphor-model.md`.

**Step 1: Write the failing tests**

In `src/phosphor/decay.rs`:

```rust
/// Fit a two-term exponential I(t) = a1*exp(-t/tau1) + a2*exp(-t/tau2)
/// to three decay data points: time to reach 10%, 1%, and 0.1% of initial.
///
/// Returns (tau_fast, tau_slow, a_fast, a_slow) where a_fast + a_slow = 1.0
pub fn fit_decay(t_10pct: f32, t_1pct: f32, t_01pct: f32) -> (f32, f32, f32, f32) {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_decay_matches_p1_data() {
        // P1: 10% at 27ms, 1% at 60ms, 0.1% at 95ms
        let (tau_fast, tau_slow, a_fast, a_slow) = fit_decay(0.027, 0.060, 0.095);

        assert!((a_fast + a_slow - 1.0).abs() < 0.001);
        assert!(tau_fast < tau_slow, "fast should be shorter than slow");
        assert!(tau_fast > 0.0);
        assert!(tau_slow > 0.0);

        // Verify the fit: evaluate at the three data points
        let i = |t: f32| a_fast * (-t / tau_fast).exp() + a_slow * (-t / tau_slow).exp();
        assert!((i(0.027) - 0.10).abs() < 0.02, "10% point: got {}", i(0.027));
        assert!((i(0.060) - 0.01).abs() < 0.005, "1% point: got {}", i(0.060));
        assert!((i(0.095) - 0.001).abs() < 0.002, "0.1% point: got {}", i(0.095));
    }

    #[test]
    fn fit_decay_matches_p7_data() {
        // P7: 10% at 305us, 1% at 5700us, 0.1% at 66000us
        let (tau_fast, tau_slow, a_fast, a_slow) = fit_decay(0.000305, 0.0057, 0.066);

        let i = |t: f32| a_fast * (-t / tau_fast).exp() + a_slow * (-t / tau_slow).exp();
        assert!((i(0.000305) - 0.10).abs() < 0.02);
        assert!((i(0.0057) - 0.01).abs() < 0.005);
        assert!((i(0.066) - 0.001).abs() < 0.002);
    }
}
```

In `src/phosphor/mod.rs`, add tests for the phosphor database:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn database_has_common_phosphors() {
        let db = phosphor_database();
        assert!(db.iter().any(|p| p.designation == "P1"));
        assert!(db.iter().any(|p| p.designation == "P7"));
        assert!(db.iter().any(|p| p.designation == "P11"));
        assert!(db.iter().any(|p| p.designation == "P31"));
    }

    #[test]
    fn dual_layer_phosphors_have_different_emission() {
        let db = phosphor_database();
        let p7 = db.iter().find(|p| p.designation == "P7").unwrap();
        assert!(p7.is_dual_layer);
        assert_ne!(
            p7.fluorescence.emission_weights,
            p7.phosphorescence.emission_weights
        );
    }

    #[test]
    fn single_layer_phosphors_share_emission() {
        let db = phosphor_database();
        let p31 = db.iter().find(|p| p.designation == "P31").unwrap();
        assert!(!p31.is_dual_layer);
        assert_eq!(
            p31.fluorescence.emission_weights,
            p31.phosphorescence.emission_weights
        );
    }

    #[test]
    fn emission_weights_are_normalized() {
        let db = phosphor_database();
        for p in &db {
            let sum_f: f32 = p.fluorescence.emission_weights.iter().sum();
            let sum_p: f32 = p.phosphorescence.emission_weights.iter().sum();
            assert!((sum_f - 1.0).abs() < 0.01, "{} fluorescence: {sum_f}", p.designation);
            assert!((sum_p - 1.0).abs() < 0.01, "{} phosphorescence: {sum_p}", p.designation);
        }
    }
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -- phosphor`
Expected: FAIL

**Step 3: Implement decay fitting**

The two-term exponential fit to 3 points is an over-determined system (3 equations, 3 unknowns given `a1 + a2 = 1`). Use a numerical approach: initial guess for `tau_fast` and `tau_slow` from the 10% and 0.1% points, then iteratively refine with a few steps of Newton's method or simple grid search. This doesn't need to be fast -- it runs once at startup.

**Step 4: Implement phosphor database**

Define `PhosphorLayer`, `PhosphorType`, `PhosphorCategory` structs in `src/phosphor/mod.rs`. Build the database as a `fn phosphor_database() -> Vec<PhosphorType>` that constructs each phosphor using `gaussian_emission_weights()` and `fit_decay()` with the data from the Tektronix tables.

Minimum set for v1: P1, P2, P3, P4, P7, P11, P14, P15, P17, P20, P24, P31, P32.

**Step 5: Run tests to verify they pass**

Run: `cargo test -- phosphor`
Expected: PASS

**Step 6: Commit**

```bash
git add src/phosphor/
git commit -m "feat: add phosphor type database with decay fitting"
```

---

### Task 3: BeamSample and Ring Buffer

**Files:**

- Create: `src/beam/mod.rs`
- Modify: `src/main.rs` (add module declaration)

**Context:** Define the common `BeamSample` type and the double-buffered ring buffer that decouples the input thread from the render thread. Read `docs/design-beam-input.md` (BeamSample section).

**Step 1: Write the failing tests**

```rust
/// A single beam position sample.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct BeamSample {
    pub x: f32,
    pub y: f32,
    pub intensity: f32,
    pub dt: f32,
}

/// Thread-safe sample queue.
/// Producer (input thread) pushes samples.
/// Consumer (render thread) drains all pending samples.
pub struct SampleBuffer { /* ... */ }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn beam_sample_is_pod() {
        let sample = BeamSample { x: 0.5, y: 0.5, intensity: 1.0, dt: 0.001 };
        let bytes = bytemuck::bytes_of(&sample);
        assert_eq!(bytes.len(), 16); // 4 x f32
    }

    #[test]
    fn sample_buffer_push_and_drain() {
        let buf = SampleBuffer::new();
        buf.push(BeamSample { x: 0.1, y: 0.2, intensity: 1.0, dt: 0.001 });
        buf.push(BeamSample { x: 0.3, y: 0.4, intensity: 0.5, dt: 0.001 });

        let drained = buf.drain();
        assert_eq!(drained.len(), 2);
        assert!((drained[0].x - 0.1).abs() < f32::EPSILON);
        assert!((drained[1].x - 0.3).abs() < f32::EPSILON);
    }

    #[test]
    fn sample_buffer_drain_clears() {
        let buf = SampleBuffer::new();
        buf.push(BeamSample { x: 0.0, y: 0.0, intensity: 1.0, dt: 0.001 });

        let first = buf.drain();
        assert_eq!(first.len(), 1);

        let second = buf.drain();
        assert_eq!(second.len(), 0);
    }

    #[test]
    fn sample_buffer_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<SampleBuffer>();
    }
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -- beam`
Expected: FAIL

**Step 3: Implement**

`SampleBuffer` wraps a `Mutex<Vec<BeamSample>>`. `push()` locks and appends. `drain()` locks and uses `std::mem::take` to swap out the vec, returning the samples. Simple, correct, lock contention is negligible since `drain()` is O(1) and happens once per frame.

**Step 4: Run tests to verify they pass**

Run: `cargo test -- beam`
Expected: PASS

**Step 5: Commit**

```bash
git add src/beam/ src/main.rs
git commit -m "feat: add BeamSample type and thread-safe sample buffer"
```

---

### Task 4: Oscilloscope Signal Generator

**Files:**

- Create: `src/beam/oscilloscope.rs`
- Modify: `src/beam/mod.rs` (add submodule)

**Context:** Built-in signal generators that produce BeamSample streams. Read `docs/design-beam-input.md` (Input Mode: Oscilloscope section).

**Step 1: Write the failing tests**

```rust
pub enum Waveform {
    Sine,
    Triangle,
    Square,
    Sawtooth,
}

pub struct ChannelConfig {
    pub waveform: Waveform,
    pub frequency: f32,   // Hz
    pub amplitude: f32,   // 0.0-1.0
    pub phase: f32,       // radians
    pub dc_offset: f32,   // -1.0-1.0
}

pub struct OscilloscopeSource {
    pub x_channel: ChannelConfig,
    pub y_channel: ChannelConfig,
    pub sample_rate: f32, // samples per simulated second
}

impl OscilloscopeSource {
    /// Generate `count` samples starting at simulated time `t_start`.
    pub fn generate(&self, t_start: f32, count: usize) -> Vec<BeamSample> {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sine_generates_correct_range() {
        let src = OscilloscopeSource {
            x_channel: ChannelConfig {
                waveform: Waveform::Sine,
                frequency: 1.0, amplitude: 1.0, phase: 0.0, dc_offset: 0.0,
            },
            y_channel: ChannelConfig {
                waveform: Waveform::Sine,
                frequency: 1.0, amplitude: 1.0,
                phase: std::f32::consts::FRAC_PI_2, dc_offset: 0.0,
            },
            sample_rate: 1000.0,
        };

        let samples = src.generate(0.0, 1000);
        assert_eq!(samples.len(), 1000);

        for s in &samples {
            assert!(s.x >= 0.0 && s.x <= 1.0, "x={} out of range", s.x);
            assert!(s.y >= 0.0 && s.y <= 1.0, "y={} out of range", s.y);
            assert!(s.intensity > 0.0);
        }
    }

    #[test]
    fn sine_cosine_makes_circle() {
        let src = OscilloscopeSource {
            x_channel: ChannelConfig {
                waveform: Waveform::Sine,
                frequency: 1.0, amplitude: 0.4, phase: 0.0, dc_offset: 0.0,
            },
            y_channel: ChannelConfig {
                waveform: Waveform::Sine,
                frequency: 1.0, amplitude: 0.4,
                phase: std::f32::consts::FRAC_PI_2, dc_offset: 0.0,
            },
            sample_rate: 1000.0,
        };

        let samples = src.generate(0.0, 1000);
        for s in &samples {
            let dx = s.x - 0.5;
            let dy = s.y - 0.5;
            let r = (dx * dx + dy * dy).sqrt();
            assert!((r - 0.4).abs() < 0.01, "r={r}, expected ~0.4");
        }
    }

    #[test]
    fn square_wave_is_binary() {
        let src = OscilloscopeSource {
            x_channel: ChannelConfig {
                waveform: Waveform::Square,
                frequency: 10.0, amplitude: 1.0, phase: 0.0, dc_offset: 0.0,
            },
            y_channel: ChannelConfig {
                waveform: Waveform::Sine,
                frequency: 1.0, amplitude: 1.0, phase: 0.0, dc_offset: 0.0,
            },
            sample_rate: 10000.0,
        };

        let samples = src.generate(0.0, 10000);
        for s in &samples {
            assert!(s.x < 0.01 || s.x > 0.99, "x={} not binary", s.x);
        }
    }

    #[test]
    fn dt_matches_sample_rate() {
        let src = OscilloscopeSource {
            x_channel: ChannelConfig {
                waveform: Waveform::Sine,
                frequency: 1.0, amplitude: 1.0, phase: 0.0, dc_offset: 0.0,
            },
            y_channel: ChannelConfig {
                waveform: Waveform::Sine,
                frequency: 1.0, amplitude: 1.0, phase: 0.0, dc_offset: 0.0,
            },
            sample_rate: 44100.0,
        };

        let samples = src.generate(0.0, 100);
        for s in &samples {
            assert!((s.dt - 1.0 / 44100.0).abs() < 1e-9);
        }
    }
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -- beam::oscilloscope`
Expected: FAIL

**Step 3: Implement**

Evaluate the waveform function at each time step `t = t_start + i / sample_rate`. Map oscillator output from `[-amplitude, +amplitude] + dc_offset` to [0, 1] screen coordinates via `(value + 1.0) / 2.0`. Set `dt = 1.0 / sample_rate` and `intensity = 1.0` (controlled externally by the UI).

**Step 4: Run tests to verify they pass**

Run: `cargo test -- beam::oscilloscope`
Expected: PASS

**Step 5: Commit**

```bash
git add src/beam/
git commit -m "feat: add oscilloscope signal generator with 4 waveforms"
```

---

### Task 5: Audio File Input

**Files:**

- Create: `src/beam/audio.rs`
- Modify: `src/beam/mod.rs` (add submodule)

**Context:** Decode stereo audio files and convert L/R samples to X/Y beam positions. Read `docs/design-beam-input.md` (Input Mode: Audio section).

**Step 1: Write the failing tests**

```rust
pub struct AudioSource {
    samples: Vec<(f32, f32)>, // (left, right)
    sample_rate: u32,
    position: usize,
}

impl AudioSource {
    pub fn load(path: &std::path::Path) -> anyhow::Result<Self> { todo!() }
    pub fn generate(&mut self, count: usize) -> Vec<BeamSample> { todo!() }
    pub fn seek(&mut self, fraction: f32) { todo!() }
    pub fn is_finished(&self) -> bool { todo!() }
    pub fn duration_secs(&self) -> f32 { todo!() }
    pub fn position_secs(&self) -> f32 { todo!() }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Helper: create a minimal WAV file with known content
    fn make_test_wav(samples: &[(f32, f32)], sample_rate: u32) -> Vec<u8> {
        let num_samples = samples.len() as u32;
        let data_size = num_samples * 2 * 4; // 2 channels, 4 bytes per f32
        let file_size = 36 + data_size;

        let mut buf = Vec::new();
        buf.extend_from_slice(b"RIFF");
        buf.extend_from_slice(&file_size.to_le_bytes());
        buf.extend_from_slice(b"WAVE");
        buf.extend_from_slice(b"fmt ");
        buf.extend_from_slice(&16u32.to_le_bytes());
        buf.extend_from_slice(&3u16.to_le_bytes());  // IEEE float
        buf.extend_from_slice(&2u16.to_le_bytes());  // stereo
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
    fn audio_source_maps_lr_to_xy() {
        let test_samples = vec![(0.0, 0.0), (1.0, -1.0), (-1.0, 1.0)];
        let wav = make_test_wav(&test_samples, 44100);
        let tmp = std::env::temp_dir().join("phosphor_test_audio.wav");
        std::fs::write(&tmp, &wav).unwrap();

        let mut src = AudioSource::load(&tmp).unwrap();
        let beams = src.generate(3);

        assert!((beams[0].x - 0.5).abs() < 0.01); // (0,0) -> (0.5, 0.5)
        assert!((beams[0].y - 0.5).abs() < 0.01);
        assert!((beams[1].x - 1.0).abs() < 0.01); // (1,-1) -> (1.0, 0.0)
        assert!((beams[1].y - 0.0).abs() < 0.01);
        assert!((beams[2].x - 0.0).abs() < 0.01); // (-1,1) -> (0.0, 1.0)
        assert!((beams[2].y - 1.0).abs() < 0.01);

        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn audio_source_dt_matches_sample_rate() {
        let wav = make_test_wav(&[(0.0, 0.0); 100], 48000);
        let tmp = std::env::temp_dir().join("phosphor_test_dt.wav");
        std::fs::write(&tmp, &wav).unwrap();

        let mut src = AudioSource::load(&tmp).unwrap();
        let beams = src.generate(10);
        for b in &beams {
            assert!((b.dt - 1.0 / 48000.0).abs() < 1e-9);
        }

        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn audio_source_seek() {
        let wav = make_test_wav(&[(0.0, 0.0); 1000], 44100);
        let tmp = std::env::temp_dir().join("phosphor_test_seek.wav");
        std::fs::write(&tmp, &wav).unwrap();

        let mut src = AudioSource::load(&tmp).unwrap();
        src.seek(0.5);
        assert!((src.position_secs() - src.duration_secs() * 0.5).abs() < 0.01);

        std::fs::remove_file(&tmp).ok();
    }
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -- beam::audio`
Expected: FAIL

**Step 3: Implement**

Use symphonia to open the file, decode all samples into `Vec<(f32, f32)>`, store with sample rate. `generate()` slices from current position and maps L/R from [-1,1] to [0,1]. Handle mono files by duplicating to both channels (or reject with a clear error for v1).

**Step 4: Run tests to verify they pass**

Run: `cargo test -- beam::audio`
Expected: PASS

**Step 5: Commit**

```bash
git add src/beam/
git commit -m "feat: add audio file input with stereo L/R to X/Y mapping"
```

---

### Task 6: Vector Display List Input

**Files:**

- Create: `src/beam/vector.rs`
- Modify: `src/beam/mod.rs` (add submodule)

**Context:** Accept a list of line segments and convert them to BeamSample streams with proper line subdivision and blanking. Read `docs/design-beam-input.md` (Input Mode: Vector Graphics section).

**Step 1: Write the failing tests**

```rust
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct VectorSegment {
    pub x0: f32, pub y0: f32,
    pub x1: f32, pub y1: f32,
    pub intensity: f32,
}

pub struct VectorSource {
    pub segments: Vec<VectorSegment>,
    pub beam_speed: f32,      // units per second (normalized coords)
    pub settling_time: f32,   // seconds for retrace between disconnected segments
}

impl VectorSource {
    /// Convert the display list to beam samples.
    /// Inserts blanked samples for retrace between disconnected segments.
    pub fn generate(&self) -> Vec<BeamSample> { todo!() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_segment_produces_samples_along_line() {
        let src = VectorSource {
            segments: vec![VectorSegment {
                x0: 0.0, y0: 0.0, x1: 1.0, y1: 0.0, intensity: 1.0,
            }],
            beam_speed: 1.0,
            settling_time: 0.001,
        };
        let samples = src.generate();
        assert!(!samples.is_empty());
        for s in &samples {
            if s.intensity > 0.0 {
                assert!((s.y).abs() < 0.01);
                assert!(s.x >= 0.0 && s.x <= 1.0);
            }
        }
    }

    #[test]
    fn disconnected_segments_have_blanked_retrace() {
        let src = VectorSource {
            segments: vec![
                VectorSegment { x0: 0.0, y0: 0.0, x1: 0.5, y1: 0.0, intensity: 1.0 },
                VectorSegment { x0: 0.5, y0: 0.5, x1: 1.0, y1: 0.5, intensity: 1.0 },
            ],
            beam_speed: 1.0,
            settling_time: 0.001,
        };
        let samples = src.generate();
        assert!(samples.iter().any(|s| s.intensity == 0.0));
    }

    #[test]
    fn connected_segments_have_no_retrace() {
        let src = VectorSource {
            segments: vec![
                VectorSegment { x0: 0.0, y0: 0.0, x1: 0.5, y1: 0.5, intensity: 1.0 },
                VectorSegment { x0: 0.5, y0: 0.5, x1: 1.0, y1: 0.0, intensity: 1.0 },
            ],
            beam_speed: 1.0,
            settling_time: 0.001,
        };
        let samples = src.generate();
        assert!(samples.iter().all(|s| s.intensity > 0.0));
    }

    #[test]
    fn longer_segments_produce_more_samples() {
        let short_src = VectorSource {
            segments: vec![VectorSegment {
                x0: 0.0, y0: 0.0, x1: 0.1, y1: 0.0, intensity: 1.0,
            }],
            beam_speed: 1.0, settling_time: 0.0,
        };
        let long_src = VectorSource {
            segments: vec![VectorSegment {
                x0: 0.0, y0: 0.0, x1: 1.0, y1: 0.0, intensity: 1.0,
            }],
            beam_speed: 1.0, settling_time: 0.0,
        };
        assert!(long_src.generate().len() > short_src.generate().len());
    }
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -- beam::vector`
Expected: FAIL

**Step 3: Implement**

Subdivide each segment into steps proportional to its length divided by beam speed. Each step becomes one `BeamSample` at the interpolated position with `dt = step_length / beam_speed`. Between disconnected segments (where `segment[i+1].start != segment[i].end`), insert blanked samples with `intensity = 0.0`.

**Step 4: Run tests to verify they pass**

Run: `cargo test -- beam::vector`
Expected: PASS

**Step 5: Commit**

```bash
git add src/beam/
git commit -m "feat: add vector display list input with line subdivision and retrace blanking"
```

---

### Task 7: External Protocol Input

**Files:**

- Create: `src/beam/external.rs`
- Modify: `src/beam/mod.rs` (add submodule)

**Context:** Parse the text-based external input protocol. Read `docs/design-beam-input.md` (Input Mode: External section).

**Step 1: Write the failing tests**

```rust
/// Parse a single line of the external protocol.
/// Returns None for empty lines, comments, and frame sync commands.
pub fn parse_line(line: &str) -> anyhow::Result<Option<BeamSample>> { todo!() }

/// Parse a line as a vector segment, returning subdivided samples.
pub fn parse_segment_line(
    line: &str,
    beam_speed: f32,
) -> anyhow::Result<Option<Vec<BeamSample>>> { todo!() }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_beam_sample() {
        let s = parse_line("B 0.5 0.75 1.0 0.001").unwrap().unwrap();
        assert!((s.x - 0.5).abs() < f32::EPSILON);
        assert!((s.y - 0.75).abs() < f32::EPSILON);
        assert!((s.intensity - 1.0).abs() < f32::EPSILON);
        assert!((s.dt - 0.001).abs() < f32::EPSILON);
    }

    #[test]
    fn parse_comment_returns_none() {
        assert!(parse_line("# this is a comment").unwrap().is_none());
    }

    #[test]
    fn parse_empty_line_returns_none() {
        assert!(parse_line("").unwrap().is_none());
        assert!(parse_line("   ").unwrap().is_none());
    }

    #[test]
    fn parse_frame_sync_returns_none() {
        assert!(parse_line("F").unwrap().is_none());
    }

    #[test]
    fn parse_invalid_returns_error() {
        assert!(parse_line("X garbage").is_err());
        assert!(parse_line("B only_two 0.5").is_err());
    }

    #[test]
    fn parse_segment_produces_samples() {
        let samples = parse_segment_line("L 0.0 0.0 1.0 0.0 1.0", 1.0)
            .unwrap().unwrap();
        assert!(!samples.is_empty());
        for s in &samples {
            assert!((s.y).abs() < 0.01);
        }
    }
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -- beam::external`
Expected: FAIL

**Step 3: Implement**

Simple string splitting and parsing. `B` lines produce a single `BeamSample`. `L` lines reuse the vector subdivision logic from `vector.rs`. `F` and comments return `None`. The actual stdin/socket reading comes later in app integration -- this task just covers parsing.

**Step 4: Run tests to verify they pass**

Run: `cargo test -- beam::external`
Expected: PASS

**Step 5: Commit**

```bash
git add src/beam/
git commit -m "feat: add external protocol parser for beam samples and segments"
```

---

### Task 8: wgpu Initialization and Window Setup

**Files:**

- Create: `src/gpu/mod.rs`
- Modify: `src/main.rs` (replace hello world with windowed app)

**Context:** Set up the winit event loop, create a window, initialize wgpu device/queue/surface. Minimal skeleton to get a window with a cleared background. Read `docs/design-gpu-pipeline.md` (Pipeline Orchestration section).

**Step 1: Implement the wgpu/winit bootstrap**

No unit tests -- this is GPU/windowing code that requires a display. Verification is visual.

`src/gpu/mod.rs`:

```rust
pub struct GpuState {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub surface: wgpu::Surface<'static>,
    pub surface_config: wgpu::SurfaceConfiguration,
}
```

`src/main.rs`: Create a winit `EventLoop`, create a window, initialize `GpuState`, run the event loop clearing to black each frame.

egui integration comes in Task 13. For now, just get the wgpu surface rendering.

**Step 2: Run the application**

Run: `cargo run`
Expected: A window appears with a black background. No crashes.

**Step 3: Commit**

```bash
git add src/gpu/ src/main.rs
git commit -m "feat: add wgpu/winit initialization with basic render loop"
```

---

### Task 9: Accumulation Buffer Textures

**Files:**

- Create: `src/gpu/accumulation.rs`
- Modify: `src/gpu/mod.rs` (add submodule)

**Context:** Create and manage the set of RGBA32Float textures that store the phosphor screen state. Read `docs/design-gpu-pipeline.md` (Accumulation Buffer section).

**Step 1: Write tests and implement**

```rust
use crate::phosphor::spectral::SPECTRAL_BANDS;

pub const TEXTURES_PER_COMPONENT: usize = (SPECTRAL_BANDS + 3) / 4;
pub const TEXTURES_PER_LAYER: usize = TEXTURES_PER_COMPONENT * 2;
pub const MAX_TEXTURES: usize = TEXTURES_PER_LAYER * 2;

pub struct AccumulationBuffer {
    pub textures: Vec<wgpu::Texture>,
    pub views: Vec<wgpu::TextureView>,
    pub width: u32,
    pub height: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texture_count_matches_bands() {
        assert_eq!(TEXTURES_PER_COMPONENT, 4);  // 16 bands / 4 channels
        assert_eq!(TEXTURES_PER_LAYER, 8);       // 4 * 2 components
        assert_eq!(MAX_TEXTURES, 16);             // 8 * 2 layers
    }
}
```

Implement `AccumulationBuffer::new()` creating the textures with `TextureFormat::Rgba32Float` and `TextureUsages::STORAGE_BINDING | TEXTURE_BINDING`.

**Step 2: Verify**

Run: `cargo test -- gpu::accumulation` for the const tests.
Run: `cargo run` to verify no crash (add a log line showing texture count and VRAM usage).

**Step 3: Commit**

```bash
git add src/gpu/
git commit -m "feat: add spectral accumulation buffer texture management"
```

---

### Task 10: Beam Write Compute Shader

**Files:**

- Create: `src/gpu/beam_write.wgsl`
- Modify: `src/gpu/mod.rs` (add compute pipeline setup and dispatch)

**Context:** The compute shader that splats beam hits into the accumulation textures as Gaussian spots. Read `docs/design-gpu-pipeline.md` (Pass 1: Beam Write) and `docs/design-beam-input.md` (Spot Profile section).

**Step 1: Write the WGSL shader**

`src/gpu/beam_write.wgsl`:

- Reads `BeamSample` array from a storage buffer
- Reads beam parameters (sigma_core, sigma_halo, halo_fraction, space_charge_k) from uniforms
- Reads phosphor emission weights and fast/slow amplitude split from uniforms
- For each sample, iterates over a tile of pixels within 4-sigma radius
- Computes the Gaussian spot profile at each pixel
- Deposits energy into the accumulation textures via `textureStore`

Workgroup size: `@workgroup_size(16, 16, 1)` for a 16x16 tile per sample.

Note: `textureStore` to Rgba32Float doesn't support atomics. If beam samples are sparse (typical), race conditions on overlapping spots produce negligible error. For dense patterns, consider a storage buffer accumulation step.

**Step 2: Set up the compute pipeline in Rust**

Create `BeamWritePipeline` struct with `wgpu::ComputePipeline`, bind group layouts, and dispatch method.

**Step 3: Integrate into frame loop**

Upload the frame's `BeamSample` batch to a GPU buffer, dispatch the beam write shader.

**Step 4: Visual verification**

Run: `cargo run`
Wire up the oscilloscope generator. Beam hits should appear as bright dots at correct positions on a black background.

**Step 5: Commit**

```bash
git add src/gpu/
git commit -m "feat: add beam write compute shader with Gaussian spot profile"
```

---

### Task 11: Decay Compute Shader

**Files:**

- Create: `src/gpu/decay.wgsl`
- Modify: `src/gpu/mod.rs` (add decay pipeline and dispatch)

**Context:** Per-texel exponential decay. Read `docs/design-gpu-pipeline.md` (Pass 2: Decay).

**Step 1: Write the WGSL shader**

`src/gpu/decay.wgsl`:

- For each texel: `value *= exp(-dt / tau)`
- `dt` (frame delta time) from a uniform buffer
- `tau` (decay constant) from a uniform buffer, per-texture
- Zero out values below `1e-6` threshold

**Step 2: Set up compute pipeline and dispatch**

One dispatch per texture, or a single dispatch iterating over bind groups.

**Step 3: Integrate into frame loop**

After beam write, dispatch decay. Textures now receive new energy AND decay existing energy each frame.

**Step 4: Visual verification**

Run: `cargo run`
Beam hits should fade over time. Fast-decay phosphors fade quickly, slow ones linger.

**Step 5: Commit**

```bash
git add src/gpu/
git commit -m "feat: add decay compute shader with per-component time constants"
```

---

### Task 12: Tonemap Fragment Shader

**Files:**

- Create: `src/gpu/tonemap.wgsl`
- Modify: `src/gpu/mod.rs` (add render pipeline)

**Context:** Convert spectral accumulation buffers to displayable sRGB. Read `docs/design-gpu-pipeline.md` (Pass 3: Tonemap / Display).

**Step 1: Write the WGSL shader**

`src/gpu/tonemap.wgsl` (full-screen triangle fragment shader):

- Sample all accumulation textures at fragment position
- Sum spectral bands across all components
- Integrate against CIE color matching functions (from uniform) to get XYZ
- XYZ to linear sRGB (3x3 matrix)
- Gamut mapping (luminance-preserving desaturation for negative channels)
- Reinhard tonemapping: `L / (1 + L)`
- sRGB gamma encode

Start with spectral-to-sRGB + tonemapping only. Bloom, glass, curvature come later.

**Step 2: Set up render pipeline**

Full-screen triangle (3 vertices, positions generated in vertex shader, no vertex buffer). Renders to swapchain surface.

**Step 3: Integrate**

Replace the clear-to-black with the tonemap pass. Frame loop: beam write -> decay -> tonemap -> present.

**Step 4: Visual verification**

Run: `cargo run`
Lissajous figure should appear in the correct phosphor color.

**Step 5: Commit**

```bash
git add src/gpu/
git commit -m "feat: add tonemap shader with spectral-to-sRGB conversion"
```

---

### Task 13: egui Integration

**Files:**

- Modify: `src/main.rs` (add egui to event loop)
- Create: `src/ui/mod.rs`
- Create: `src/ui/scope_panel.rs`

**Context:** Integrate egui as an overlay on top of the CRT output. Minimal scope panel with phosphor selector and intensity slider. Read `docs/design-ui.md`.

**Step 1: Set up egui-winit and egui-wgpu**

- Create `egui::Context`, `egui_winit::State`, `egui_wgpu::Renderer`
- Pass winit events to egui_winit
- Each frame: begin egui frame, build UI, end frame, render egui on top of CRT output

**Step 2: Build minimal scope panel**

`src/ui/scope_panel.rs`:

- `egui::SidePanel` with phosphor type dropdown, intensity slider, focus slider, FPS display

**Step 3: Wire controls to simulation state**

Changing phosphor type updates emission weights and decay constants. Intensity scales beam energy.

**Step 4: Visual verification**

Run: `cargo run`
CRT on right, egui panel on left. Changing phosphor changes color. Intensity changes brightness.

**Step 5: Commit**

```bash
git add src/ui/ src/main.rs
git commit -m "feat: add egui side panel with phosphor selector and intensity control"
```

---

### Task 14: Faceplate Scatter Post-Processing

**Files:**

- Create: `src/gpu/faceplate_scatter_downsample.wgsl` (threshold downsample shader)
- Create: `src/gpu/faceplate_scatter_blur.wgsl` (separable Gaussian blur shader)
- Create: `src/gpu/faceplate_scatter.rs` (FaceplateScatterPipeline, textures, params)
- Modify: `src/gpu/composite.wgsl` (add scatter texture input)
- Modify: `src/gpu/composite.rs` (add scatter texture bind group)
- Modify: `src/gpu/mod.rs` (add faceplate_scatter pipeline, wire into render loop)

**Context:** Faceplate scatter — light scattering through the glass faceplate before exiting. Slots between spectral_resolve and composite in the two-stage display pipeline.

**Architecture:**

Pipeline: `spectral_resolve → HDR texture → [scatter downsample → blur H → blur V] → composite (HDR + scatter → tonemap → swapchain)`

- Two Rgba16Float textures at half resolution (scatter_a, scatter_b) for ping-pong blur
- Downsample reads HDR texture via `textureLoad` (manual 4-tap average since Rgba32Float isn't filterable), applies brightness threshold
- Separable Gaussian blur with sigma uniform, kernel cutoff at 3σ, weights computed in shader
- Composite shader receives scatter texture as second input, adds `scatter * intensity` before exposure+tonemapping

**DONE** — Implemented and committed.

---

### Task 15: Glass and Curvature Effects

**Files:**

- Modify: `src/gpu/tonemap.wgsl`
- Create: `src/ui/engineer_panel.rs`

**Context:** Faceplate tint, screen curvature, edge darkening. Read `docs/design-gpu-pipeline.md` (Stage 4: Glass and Curvature).

**Step 1: Add glass tint**

Multiply linear RGB by configurable tint color (uniform). Default: slight gray-green `(0.92, 0.95, 0.92)`.

**Step 2: Add screen curvature**

UV distortion in fragment shader. Barrel distortion parameterized by curvature radius. Pixels outside the curved area render black (bezel).

**Step 3: Add edge darkening**

`cos(theta)` falloff from screen center, simulating Lambertian viewing angle.

**Step 4: Add engineer mode panel**

`src/ui/engineer_panel.rs` with glass tint color picker, curvature radius slider, edge falloff slider. Tab switcher between scope and engineer panels.

**Step 5: Visual verification**

Run: `cargo run`
Subtle curvature, tinted glass, dimmer edges. Controls adjust in real time.

**Step 6: Commit**

```bash
git add src/gpu/ src/ui/
git commit -m "feat: add glass tint, screen curvature, and edge darkening"
```

---

### Task 16: Full Engineer Mode Panel

**Files:**

- Modify: `src/ui/engineer_panel.rs`
- Modify: `src/ui/mod.rs`

**Context:** Expose all physics parameters. Read `docs/design-ui.md` (Engineer Mode Tab section).

**Step 1: Add beam parameter controls**

sigma_core, sigma_halo, halo fraction, space charge coefficient, acceleration voltage sliders.

**Step 2: Add phosphor override controls**

tau_fast, tau_slow sliders. A_fast/A_slow ratio slider. Small bar chart of the 16 spectral band emission weights.

**Step 3: Add display pipeline controls**

Tonemap curve selector (Reinhard/Filmic/Exposure). Exposure, white point. Accumulation buffer resolution multiplier.

**Step 4: Add performance stats**

FPS, per-pass GPU timing (if timestamp queries available), beam samples per frame.

**Step 5: Visual verification**

Run: `cargo run`
Engineer tab adjusts beam/decay/display parameters visibly. Performance stats update.

**Step 6: Commit**

```bash
git add src/ui/
git commit -m "feat: add full engineer mode panel with all physics parameters"
```

---

### Task 17: Input Mode Switching and Scope Panel Completion

**Files:**

- Modify: `src/ui/scope_panel.rs`
- Create: `src/app.rs` (central app state if not yet created)

**Context:** Wire up all four input modes with per-mode controls. Read `docs/design-ui.md` (Scope Mode Tab section).

**Step 1: Add input mode selector**

Radio buttons: Oscilloscope, Audio, Vector, External.

**Step 2: Add per-mode controls**

Oscilloscope: waveform dropdowns, frequency/amplitude/phase/offset sliders, timebase.
Audio: file picker, transport (play/pause/stop), seek bar, loop, speed.
Vector: load file button, refresh toggle.
External: mode (stdin/socket), socket path, connection status.

**Step 3: Visual verification**

Run: `cargo run`
Switch between modes. Oscilloscope shows Lissajous. Audio file visualizes. Vector mode renders display lists.

**Step 4: Commit**

```bash
git add src/ui/ src/app.rs
git commit -m "feat: complete scope panel with all input mode controls"
```

---

### Task 18: Multi-Window Support

**Files:**

- Modify: `src/main.rs`
- Modify: `src/ui/mod.rs`

**Context:** Detachable control panel as a separate OS window. Read `docs/design-ui.md` (Window Layouts section).

**Step 1: Multi-window event loop**

Support creating a second winit window. CRT viewport window owns the wgpu surface and full GPU pipeline. Controls window runs egui only (lightweight wgpu surface for egui rendering).

**Step 2: Toggle hotkey**

`Ctrl+D` toggles combined/detached. Creates or destroys the second window.

**Step 3: Shared state**

Both windows read/write the same `AppState` behind `Arc<Mutex<>>`.

**Step 4: Visual verification**

Run: `cargo run`
`Ctrl+D` pops controls into separate window. CRT continues rendering. `Ctrl+D` again recombines.

**Step 5: Commit**

```bash
git add src/
git commit -m "feat: add detachable control panel with multi-window support"
```

---

### Task 19: Graticule Overlay

**Files:**

- Create: `src/ui/viewport.rs`

**Context:** Optional oscilloscope graticule (8x10 grid). Read `docs/design-ui.md` (Scope Mode Tab section).

**Step 1: Implement graticule**

Render as an egui paint overlay -- thin dashed lines over the CRT output. Resolution-independent.

**Step 2: Add toggle**

Checkbox in scope panel, only visible in oscilloscope mode.

**Step 3: Visual verification**

Run: `cargo run`
Graticule appears as dashed 8x10 grid with configurable color/opacity.

**Step 4: Commit**

```bash
git add src/ui/
git commit -m "feat: add optional graticule overlay for oscilloscope mode"
```

---

### Task 20: Polish and Integration Testing

**Files:** Various

**Context:** Final integration pass.

**Step 1: End-to-end testing**

- Load each phosphor type, verify colors match reference data
- Oscilloscope with Lissajous patterns
- Load an oscilloscope music audio file
- Vector display list with Asteroids-style pattern
- External protocol: `echo "B 0.5 0.5 1.0 0.01" | cargo run`
- Switch input modes rapidly
- Toggle detached mode while running
- No GPU validation errors: `WGPU_BACKEND_TYPE=vulkan RUST_LOG=wgpu=warn cargo run`

**Step 2: Performance profiling**

- Frame time at 1080p and 4K with P7 (long decay, dual layer -- worst case)
- Beam write bottleneck check with large sample batches
- VRAM usage vs expectations

**Step 3: Fix issues**

Address bugs, glitches, performance problems.

**Step 4: Commit**

```bash
git add -A
git commit -m "chore: polish, integration testing, and performance tuning"
```
