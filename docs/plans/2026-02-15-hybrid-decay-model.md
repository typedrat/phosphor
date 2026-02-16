# Hybrid Phosphor Decay Model Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace the uniform bi-exponential phosphor decay with a three-tier hybrid model (instantaneous exponentials, slow multiplicative exponentials, power-law with elapsed-time tracking) based on measured data from Kuhn (2002) and Selomulya (2003).

**Architecture:** The decay pipeline splits into three tiers: Tier 1 (tau << frame dt) computes fast exponentials analytically in the beam write pass. Tier 2 (slow exponentials) stays multiplicative in the accumulation buffer. Tier 3 (power-law) tracks per-texel elapsed time. The `PhosphorLayer` struct changes from fixed `tau_fast/tau_slow` to a `Vec<DecayTerm>` with up to 8 terms. The accumulation buffer becomes variable-size based on the active phosphor's classified term counts.

**Tech Stack:** Rust, wgpu 27, WGSL compute/fragment shaders, serde/toml, bytemuck, proc-macro

---

### Task 1: Add DecayTerm enum to phosphor-data crate

**Files:**

- Modify: `crates/phosphor-data/src/lib.rs` (DecayTerm enum, PhosphorLayer changes)
- Test: `crates/phosphor-data/src/lib.rs` (inline tests)

**Step 1: Write the failing test**

Add to the `#[cfg(test)]` section of `crates/phosphor-data/src/lib.rs` (which doesn't exist yet -- add one):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decay_term_exponential_fields() {
        let term = DecayTerm::Exponential { amplitude: 6.72, tau: 0.00288 };
        match term {
            DecayTerm::Exponential { amplitude, tau } => {
                assert!((amplitude - 6.72).abs() < 1e-6);
                assert!((tau - 0.00288).abs() < 1e-8);
            }
            _ => panic!("expected Exponential"),
        }
    }

    #[test]
    fn decay_term_power_law_fields() {
        let term = DecayTerm::PowerLaw { amplitude: 2.1e-4, alpha: 5.5e-6, beta: 1.1 };
        match term {
            DecayTerm::PowerLaw { amplitude, alpha, beta } => {
                assert!((amplitude - 2.1e-4).abs() < 1e-10);
                assert!((alpha - 5.5e-6).abs() < 1e-12);
                assert!((beta - 1.1).abs() < 1e-6);
            }
            _ => panic!("expected PowerLaw"),
        }
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p phosphor-data`
Expected: FAIL -- `DecayTerm` not defined.

**Step 3: Write the DecayTerm enum and update PhosphorLayer**

In `crates/phosphor-data/src/lib.rs`, add the enum before the existing types:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(tag = "type")]
pub enum DecayTerm {
    #[serde(rename = "exponential")]
    Exponential { amplitude: f32, tau: f32 },
    #[serde(rename = "power_law")]
    PowerLaw { amplitude: f32, alpha: f32, beta: f32 },
}
```

Update `PhosphorLayer` to replace fixed decay fields with `decay_terms`:

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct PhosphorLayer {
    pub emission_weights: [f32; SPECTRAL_BANDS],
    pub decay_terms: Vec<DecayTerm>,
}
```

Remove the `tau_fast`, `tau_slow`, `a_fast`, `a_slow` fields.

Also add the public re-export: `pub use DecayTerm;` is already covered since it's in the same module.

**Step 4: Run test to verify it passes**

Run: `cargo test -p phosphor-data`
Expected: The new tests pass. Other tests and downstream code will fail -- that's expected, we'll fix them in subsequent tasks.

**Step 5: Commit**

```bash
git add crates/phosphor-data/src/lib.rs
git commit -m "feat: add DecayTerm enum, update PhosphorLayer to use decay_terms vec"
```

---

### Task 2: Add tier classification helper to phosphor-data

**Files:**

- Modify: `crates/phosphor-data/src/lib.rs`

This helper classifies a phosphor's decay terms into tiers for the GPU pipeline.

**Step 1: Write the failing test**

```rust
#[test]
fn classify_p1_all_slow_exponential() {
    let terms = vec![
        DecayTerm::Exponential { amplitude: 6.72, tau: 0.00288 },
        DecayTerm::Exponential { amplitude: 1.0, tau: 0.0151 },
    ];
    let class = classify_decay_terms(&terms, 1e-4);
    assert_eq!(class.instant_exp_count, 0);
    assert_eq!(class.slow_exp_count, 2);
    assert!(!class.has_power_law);
}

#[test]
fn classify_p31_power_law_plus_instant() {
    let terms = vec![
        DecayTerm::PowerLaw { amplitude: 2.1e-4, alpha: 5.5e-6, beta: 1.1 },
        DecayTerm::Exponential { amplitude: 90.0, tau: 31.8e-9 },
        DecayTerm::Exponential { amplitude: 100.0, tau: 227e-9 },
        DecayTerm::Exponential { amplitude: 37.0, tau: 1.06e-6 },
    ];
    let class = classify_decay_terms(&terms, 1e-4);
    assert_eq!(class.instant_exp_count, 3);
    assert_eq!(class.slow_exp_count, 0);
    assert!(class.has_power_law);
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p phosphor-data`

**Step 3: Write the classifier**

```rust
/// Result of classifying a phosphor's decay terms into tiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecayClassification {
    /// Exponential terms with tau < tau_cutoff (tier 1: computed analytically).
    pub instant_exp_count: usize,
    /// Exponential terms with tau >= tau_cutoff (tier 2: multiplicative per-frame).
    pub slow_exp_count: usize,
    /// Whether any power-law term exists (tier 3: elapsed-time tracking).
    pub has_power_law: bool,
}

impl DecayClassification {
    /// Total accumulation buffer layers needed for this phosphor layer.
    /// Tier 2: slow_exp_count x SPECTRAL_BANDS
    /// Tier 3: SPECTRAL_BANDS (peak energy) + 1 (elapsed time) if has_power_law
    pub fn accum_layers(&self) -> usize {
        let mut layers = self.slow_exp_count * SPECTRAL_BANDS;
        if self.has_power_law {
            layers += SPECTRAL_BANDS + 1;
        }
        layers
    }
}

/// Classify decay terms into tiers based on a time constant cutoff.
/// Terms with tau < tau_cutoff are "instant" (tier 1).
/// Terms with tau >= tau_cutoff are "slow" (tier 2).
/// Power-law terms are always tier 3.
pub fn classify_decay_terms(terms: &[DecayTerm], tau_cutoff: f32) -> DecayClassification {
    let mut instant = 0;
    let mut slow = 0;
    let mut power_law = false;

    for term in terms {
        match term {
            DecayTerm::Exponential { tau, .. } => {
                if *tau < tau_cutoff {
                    instant += 1;
                } else {
                    slow += 1;
                }
            }
            DecayTerm::PowerLaw { .. } => {
                power_law = true;
            }
        }
    }

    DecayClassification {
        instant_exp_count: instant,
        slow_exp_count: slow,
        has_power_law: power_law,
    }
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p phosphor-data`
Expected: PASS for classification tests.

**Step 5: Commit**

```bash
git add crates/phosphor-data/src/lib.rs
git commit -m "feat: add decay tier classification (instant/slow/power-law)"
```

---

### Task 3: Update TOML format and deserialization

**Files:**

- Modify: `data/phosphors.toml`
- Modify: `crates/phosphor-data/src/lib.rs` (PhosphorData, build_phosphor)

**Step 1: Write the failing test**

```rust
#[test]
fn parse_explicit_decay_terms_from_toml() {
    let toml_str = r#"
[P1]
description = "Medium persistence green."
category = "general_purpose"
peak_nm = 520.0
fwhm_nm = 40.0
relative_luminance = 50.0
relative_writing_speed = 60.0

[[P1.decay_terms]]
type = "exponential"
amplitude = 6.72
tau = 0.00288

[[P1.decay_terms]]
type = "exponential"
amplitude = 1.0
tau = 0.0151
"#;
    let phosphors = load_phosphors(toml_str).unwrap();
    assert_eq!(phosphors.len(), 1);
    let p1 = &phosphors[0];
    assert_eq!(p1.fluorescence.decay_terms.len(), 2);
    match p1.fluorescence.decay_terms[0] {
        DecayTerm::Exponential { amplitude, tau } => {
            assert!((amplitude - 6.72).abs() < 1e-6);
            assert!((tau - 0.00288).abs() < 1e-8);
        }
        _ => panic!("expected exponential"),
    }
}

#[test]
fn parse_power_law_term_from_toml() {
    let toml_str = r#"
[P31]
description = "Medium-short persistence green."
category = "general_purpose"
peak_nm = 530.0
fwhm_nm = 50.0
relative_luminance = 100.0
relative_writing_speed = 100.0

[[P31.decay_terms]]
type = "power_law"
amplitude = 2.1e-4
alpha = 5.5e-6
beta = 1.1

[[P31.decay_terms]]
type = "exponential"
amplitude = 90.0
tau = 31.8e-9
"#;
    let phosphors = load_phosphors(toml_str).unwrap();
    let p31 = &phosphors[0];
    assert_eq!(p31.fluorescence.decay_terms.len(), 2);
    match p31.fluorescence.decay_terms[0] {
        DecayTerm::PowerLaw { amplitude, alpha, beta } => {
            assert!((amplitude - 2.1e-4).abs() < 1e-10);
            assert!((alpha - 5.5e-6).abs() < 1e-12);
            assert!((beta - 1.1).abs() < 1e-6);
        }
        _ => panic!("expected power_law"),
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p phosphor-data`

**Step 3: Update the TOML deserialization structs**

In `crates/phosphor-data/src/lib.rs`, update `PhosphorData` and `LayerData`:

```rust
#[derive(Debug, Deserialize)]
struct LayerData {
    peak_nm: f32,
    fwhm_nm: f32,
    #[serde(default)]
    decay_terms: Vec<DecayTerm>,
}

#[derive(Debug, Deserialize)]
struct PhosphorData {
    description: String,
    category: String,
    #[serde(default)]
    dual_layer: bool,
    peak_nm: f32,
    #[serde(default)]
    fwhm_nm: Option<f32>,
    #[serde(default)]
    decay_terms: Vec<DecayTerm>,
    relative_luminance: f32,
    relative_writing_speed: f32,
    fluorescence: Option<LayerData>,
    phosphorescence: Option<LayerData>,
}
```

Remove `t_10pct`, `t_1pct`, `t_01pct` from `PhosphorData`.

Update `build_phosphor` to use explicit decay terms:

```rust
fn build_phosphor(designation: &str, data: &PhosphorData) -> PhosphorType {
    let make_layer = |peak: f32, fwhm: f32, terms: &[DecayTerm]| -> PhosphorLayer {
        PhosphorLayer {
            emission_weights: spectral::gaussian_emission_weights(peak, fwhm),
            decay_terms: terms.to_vec(),
        }
    };

    let (fluorescence, phosphorescence, is_dual_layer) = if data.dual_layer {
        let fl = data.fluorescence.as_ref().unwrap_or_else(|| {
            panic!("{designation}: dual_layer = true but missing [fluorescence]")
        });
        let ph = data.phosphorescence.as_ref().unwrap_or_else(|| {
            panic!("{designation}: dual_layer = true but missing [phosphorescence]")
        });
        let fl_terms = if fl.decay_terms.is_empty() {
            &data.decay_terms
        } else {
            &fl.decay_terms
        };
        let ph_terms = if ph.decay_terms.is_empty() {
            &data.decay_terms
        } else {
            &ph.decay_terms
        };
        (
            make_layer(fl.peak_nm, fl.fwhm_nm, fl_terms),
            make_layer(ph.peak_nm, ph.fwhm_nm, ph_terms),
            true,
        )
    } else {
        let fwhm = data
            .fwhm_nm
            .unwrap_or_else(|| panic!("{designation}: single-layer phosphor missing fwhm_nm"));
        let layer = make_layer(data.peak_nm, fwhm, &data.decay_terms);
        (layer.clone(), layer, false)
    };

    PhosphorType {
        designation: designation.to_string(),
        description: data.description.clone(),
        category: parse_category(&data.category),
        is_dual_layer,
        fluorescence,
        phosphorescence,
        peak_wavelength_nm: data.peak_nm,
        relative_luminance: data.relative_luminance,
        relative_writing_speed: data.relative_writing_speed,
    }
}
```

**Step 4: Update `data/phosphors.toml` with explicit decay terms**

Replace all entries with explicit decay terms. Use Kuhn/Selomulya data where available; use LM-fitted bi-exponential parameters (extracted once before removing the fitter) for the rest. See design doc section "Phosphor Data Values" for the specific values.

**Step 5: Run tests**

Run: `cargo test -p phosphor-data`
Expected: All new parsing tests pass.

**Step 6: Commit**

```bash
git add crates/phosphor-data/src/lib.rs data/phosphors.toml
git commit -m "feat: switch phosphor TOML to explicit decay terms, remove LM fitting"
```

---

### Task 4: Remove levenberg-marquardt/nalgebra dependencies

**Files:**

- Delete content of: `crates/phosphor-data/src/decay.rs`
- Modify: `crates/phosphor-data/Cargo.toml`
- Modify: `crates/phosphor-data/src/lib.rs`

**Step 1: Remove the decay fitting module**

Replace `crates/phosphor-data/src/decay.rs` with an empty file or remove `pub mod decay;` from lib.rs. The `fit_decay` function and `DecayFitProblem` struct are no longer needed.

**Step 2: Remove dependencies from Cargo.toml**

Remove `levenberg-marquardt` and `nalgebra` from `crates/phosphor-data/Cargo.toml`.

**Step 3: Run full build**

Run: `cargo build -p phosphor-data`
Expected: Clean build with no fitting code.

**Step 4: Run tests**

Run: `cargo test -p phosphor-data`
Expected: All tests pass (the old `fit_decay_matches_*` tests are gone).

**Step 5: Commit**

```bash
git add crates/phosphor-data/
git commit -m "chore: remove levenberg-marquardt/nalgebra, decay fitting no longer needed"
```

---

### Task 5: Update proc macro for new PhosphorLayer format

**Files:**

- Modify: `crates/phosphor-data-macro/src/lib.rs`

**Step 1: Write the failing test**

Run: `cargo build` (the main crate uses the proc macro -- it will fail to compile because `format_layer` still references `tau_fast`/`tau_slow`/`a_fast`/`a_slow`).

**Step 2: Update format_layer**

```rust
fn format_decay_term(term: &phosphor_data::DecayTerm) -> String {
    match term {
        phosphor_data::DecayTerm::Exponential { amplitude, tau } => {
            format!(
                "phosphor_data::DecayTerm::Exponential {{ amplitude: {amplitude}_f32, tau: {tau}_f32 }}"
            )
        }
        phosphor_data::DecayTerm::PowerLaw { amplitude, alpha, beta } => {
            format!(
                "phosphor_data::DecayTerm::PowerLaw {{ amplitude: {amplitude}_f32, alpha: {alpha}_f32, beta: {beta}_f32 }}"
            )
        }
    }
}

fn format_layer(layer: &phosphor_data::PhosphorLayer) -> String {
    let terms: Vec<String> = layer.decay_terms.iter().map(format_decay_term).collect();
    let terms_str = terms.join(", ");
    format!(
        "phosphor_data::PhosphorLayer {{ emission_weights: {}, decay_terms: vec![{}] }}",
        format_weights(&layer.emission_weights),
        terms_str,
    )
}
```

**Step 3: Build and run tests**

Run: `cargo build && cargo test -p phosphor`
Expected: Compiles. The `phosphor_database()` returns phosphors with the new `decay_terms` field.

**Step 4: Update existing phosphor tests in `src/phosphor/mod.rs`**

The tests that check `emission_weights` should still work. Tests that reference `tau_fast`/`tau_slow` need updating to check `decay_terms` instead.

**Step 5: Commit**

```bash
git add crates/phosphor-data-macro/src/lib.rs src/phosphor/mod.rs
git commit -m "feat: update proc macro to generate DecayTerm vectors"
```

---

### Task 6: Update AccumulationBuffer for variable layer counts

**Files:**

- Modify: `src/gpu/accumulation.rs`
- Modify: `src/gpu/mod.rs` (where AccumulationBuffer is constructed)

**Step 1: Write the failing test**

In `src/gpu/accumulation.rs`, update the test:

```rust
#[test]
fn p1_layer_count() {
    // P1: 2 slow exponentials x 16 bands = 32 layers
    assert_eq!(accum_layer_count(2, false), 32);
}

#[test]
fn p31_layer_count() {
    // P31: 0 slow exp, 1 power law -> 16 peak + 1 elapsed = 17
    assert_eq!(accum_layer_count(0, true), 17);
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p phosphor accumulation`

**Step 3: Implement variable layer count**

Replace the fixed `LAYERS_PER_COMPONENT` / `LAYERS_PER_DECAY_PAIR` constants:

```rust
use phosphor_data::spectral::SPECTRAL_BANDS;

/// Compute total accumulation buffer layers given a decay classification.
/// Tier 2: slow_exp_count x SPECTRAL_BANDS
/// Tier 3: SPECTRAL_BANDS + 1 if has_power_law (peak energy + elapsed time)
pub fn accum_layer_count(slow_exp_count: usize, has_power_law: bool) -> u32 {
    let mut layers = slow_exp_count * SPECTRAL_BANDS;
    if has_power_law {
        layers += SPECTRAL_BANDS + 1;
    }
    layers as u32
}
```

Update `AccumulationBuffer::new()` to take `layers: u32` directly instead of computing from `phosphor_layers`:

```rust
pub fn new(device: &wgpu::Device, resolution: Resolution, layers: u32) -> Self {
    // ... same as before but using layers directly
}
```

Update `resize` similarly.

**Step 4: Update GpuState::new() in `src/gpu/mod.rs`**

Where it currently does:

```rust
let accum = AccumulationBuffer::new(&device, buffer_res, 1);
```

Change to compute layers from the default phosphor's classification:

```rust
let default_phosphor = &phosphors[0]; // passed in or computed
let class = phosphor_data::classify_decay_terms(
    &default_phosphor.fluorescence.decay_terms, TAU_CUTOFF);
let accum = AccumulationBuffer::new(&device, buffer_res, class.accum_layers() as u32);
```

Define `pub const TAU_CUTOFF: f32 = 1e-4;` (100 us) in `src/gpu/mod.rs`.

**Step 5: Run tests**

Run: `cargo test`
Expected: Layer count tests pass. Build may have further failures from shader uniform changes -- those are next.

**Step 6: Commit**

```bash
git add src/gpu/accumulation.rs src/gpu/mod.rs
git commit -m "feat: variable accumulation buffer layer count from decay classification"
```

---

### Task 7: Update DecayParams GPU struct and decay.wgsl shader

**Files:**

- Modify: `src/gpu/decay.rs`
- Modify: `src/gpu/decay.wgsl`

**Step 1: Update Rust DecayParams**

Replace the fixed `tau_fast/tau_slow` struct with the new tier-aware struct:

```rust
pub const MAX_DECAY_TERMS: usize = 8;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct DecayTermGpu {
    pub amplitude: f32,
    pub param1: f32,    // tau (exp) or alpha (power_law)
    pub param2: f32,    // 0.0 (exp) or beta (power_law)
    pub type_flag: f32, // 0.0 = exponential, 1.0 = power_law
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct DecayParams {
    pub dt: f32,
    pub threshold: f32,
    pub tau_cutoff: f32,
    pub term_count: u32,
    pub terms: [DecayTermGpu; MAX_DECAY_TERMS],
    pub slow_exp_count: u32,
    pub has_power_law: u32,
    pub _pad0: u32,
    pub _pad1: u32,
}
```

Add a constructor that takes `&[DecayTerm]` and a `tau_cutoff`:

```rust
impl DecayParams {
    pub fn from_terms(terms: &[phosphor_data::DecayTerm], tau_cutoff: f32) -> Self {
        let mut gpu_terms = [DecayTermGpu::zeroed(); MAX_DECAY_TERMS];
        let class = phosphor_data::classify_decay_terms(terms, tau_cutoff);

        // Pack slow exponentials first (tier 2), then power-law (tier 3)
        let mut idx = 0;
        for term in terms {
            if let phosphor_data::DecayTerm::Exponential { amplitude, tau } = term {
                if *tau >= tau_cutoff {
                    gpu_terms[idx] = DecayTermGpu {
                        amplitude: *amplitude,
                        param1: *tau,
                        param2: 0.0,
                        type_flag: 0.0,
                    };
                    idx += 1;
                }
            }
        }
        for term in terms {
            if let phosphor_data::DecayTerm::PowerLaw { amplitude, alpha, beta } = term {
                gpu_terms[idx] = DecayTermGpu {
                    amplitude: *amplitude,
                    param1: *alpha,
                    param2: *beta,
                    type_flag: 1.0,
                };
                idx += 1;
            }
        }

        Self {
            dt: 0.0,
            threshold: 1e-6,
            tau_cutoff,
            term_count: idx as u32,
            terms: gpu_terms,
            slow_exp_count: class.slow_exp_count as u32,
            has_power_law: if class.has_power_law { 1 } else { 0 },
            _pad0: 0,
            _pad1: 0,
        }
    }

    pub fn with_dt(mut self, dt: f32) -> Self {
        self.dt = dt;
        self
    }
}
```

**Step 2: Update decay.wgsl**

```wgsl
override SPECTRAL_BANDS: u32 = 16u;

struct DecayTermGpu {
    amplitude: f32,
    param1: f32,
    param2: f32,
    type_flag: f32,
}

struct DecayParams {
    dt: f32,
    threshold: f32,
    tau_cutoff: f32,
    term_count: u32,
    terms: array<DecayTermGpu, 8>,
    slow_exp_count: u32,
    has_power_law: u32,
    _pad0: u32,
    _pad1: u32,
}

struct AccumDims {
    width: u32,
    height: u32,
    layers: u32,
    _pad: u32,
}

@group(0) @binding(0) var<uniform> params: DecayParams;
@group(1) @binding(0) var<storage, read_write> accum: array<u32>;
@group(1) @binding(1) var<uniform> accum_dims: AccumDims;

fn accum_index(x: i32, y: i32, layer: u32) -> u32 {
    return layer * (accum_dims.width * accum_dims.height)
         + u32(y) * accum_dims.width + u32(x);
}

fn load_accum(x: i32, y: i32, layer: u32) -> f32 {
    return bitcast<f32>(accum[accum_index(x, y, layer)]);
}

fn store_accum(x: i32, y: i32, layer: u32, val: f32) {
    accum[accum_index(x, y, layer)] = bitcast<u32>(val);
}

@compute @workgroup_size(16, 16, 1)
fn main(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let coord = vec2<i32>(global_id.xy);

    if coord.x >= i32(accum_dims.width) || coord.y >= i32(accum_dims.height) {
        return;
    }

    let threshold = params.threshold;

    // Tier 2: slow exponentials -- multiplicative decay
    for (var term = 0u; term < params.slow_exp_count; term++) {
        let tau = params.terms[term].param1;
        let factor = exp(-params.dt / tau);
        for (var band = 0u; band < SPECTRAL_BANDS; band++) {
            let layer = term * SPECTRAL_BANDS + band;
            let val = load_accum(coord.x, coord.y, layer);
            let decayed = val * factor;
            store_accum(coord.x, coord.y, layer,
                select(decayed, 0.0, decayed < threshold));
        }
    }

    // Tier 3: power-law -- elapsed time tracking
    if params.has_power_law == 1u {
        let base = params.slow_exp_count * SPECTRAL_BANDS;
        let time_layer = base + SPECTRAL_BANDS;

        var elapsed = load_accum(coord.x, coord.y, time_layer);
        elapsed += params.dt;
        store_accum(coord.x, coord.y, time_layer, elapsed);

        // Find the power-law term (first one with type_flag == 1.0)
        for (var i = 0u; i < params.term_count; i++) {
            if params.terms[i].type_flag == 1.0 {
                let alpha = params.terms[i].param1;
                let beta = params.terms[i].param2;

                // Threshold dead texels to save compute
                for (var band = 0u; band < SPECTRAL_BANDS; band++) {
                    let peak = load_accum(coord.x, coord.y, base + band);
                    if peak > 0.0 {
                        let value = peak
                            * pow(alpha / (elapsed + alpha), beta);
                        if value < threshold {
                            store_accum(coord.x, coord.y, base + band, 0.0);
                        }
                    }
                }
                break;
            }
        }
    }
}
```

**Step 3: Build**

Run: `cargo build`
Expected: Compiles (shader + Rust struct changes consistent).

**Step 4: Commit**

```bash
git add src/gpu/decay.rs src/gpu/decay.wgsl
git commit -m "feat: three-tier decay shader (slow exponential + power-law elapsed time)"
```

---

### Task 8: Update beam_write for tier-aware energy deposition

**Files:**

- Modify: `src/gpu/beam_write.rs` (EmissionParams struct)
- Modify: `src/gpu/beam_write.wgsl`

The beam write shader deposits energy into tier-2 and tier-3 layers. Tier-1 instantaneous emission is folded into the deposited energy as a brightness boost (see design doc rationale).

**Step 1: Update EmissionParams to carry tier info**

Replace `fast_fraction` with tier counts and instant energy scale:

```rust
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct EmissionParams {
    pub weights: [[f32; 4]; 4],        // 16 spectral band weights
    pub slow_exp_count: u32,
    pub has_power_law: u32,
    pub instant_energy_scale: f32,     // sum of A*tau for tier-1 terms
    pub _pad: f32,
}

impl EmissionParams {
    pub fn from_phosphor(
        weights: &[f32; 16],
        terms: &[phosphor_data::DecayTerm],
        tau_cutoff: f32,
    ) -> Self {
        let mut packed = [[0.0f32; 4]; 4];
        for (i, &w) in weights.iter().enumerate() {
            packed[i / 4][i % 4] = w;
        }

        let class = phosphor_data::classify_decay_terms(terms, tau_cutoff);

        // Compute instant energy scale: sum of A*tau for tier-1 terms
        let instant_scale: f32 = terms.iter().filter_map(|t| match t {
            phosphor_data::DecayTerm::Exponential { amplitude, tau }
                if *tau < tau_cutoff => Some(amplitude * tau),
            _ => None,
        }).sum();

        Self {
            weights: packed,
            slow_exp_count: class.slow_exp_count as u32,
            has_power_law: if class.has_power_law { 1 } else { 0 },
            instant_energy_scale: instant_scale,
            _pad: 0.0,
        }
    }
}
```

**Step 2: Update beam_write.wgsl**

Replace the inner band loop:

```wgsl
struct EmissionParams {
    weights: array<vec4<f32>, 4>,
    slow_exp_count: u32,
    has_power_law: u32,
    instant_energy_scale: f32,
    _pad: f32,
}

// In main(), replace the energy deposition loop:

let base_energy = sample.intensity * profile_val * sample.dt;

// Include tier-1 instantaneous contribution as brightness boost
let boosted_energy = base_energy * (1.0 + emission.instant_energy_scale);

// Tier 2: deposit into slow exponential layers
for (var term = 0u; term < emission.slow_exp_count; term++) {
    for (var band = 0u; band < SPECTRAL_BANDS; band++) {
        let energy = boosted_energy * get_emission_weight(band);
        let layer = term * SPECTRAL_BANDS + band;
        atomic_add_f32(accum_index(px_x, px_y, layer), energy);
    }
}

// Tier 3: deposit peak energy into power-law layers, reset elapsed time
if emission.has_power_law == 1u {
    let pl_base = emission.slow_exp_count * SPECTRAL_BANDS;
    for (var band = 0u; band < SPECTRAL_BANDS; band++) {
        let energy = boosted_energy * get_emission_weight(band);
        atomic_add_f32(accum_index(px_x, px_y, pl_base + band), energy);
    }
    // Reset elapsed time to 0 for this texel
    let time_layer = pl_base + SPECTRAL_BANDS;
    accum[accum_index(px_x, px_y, time_layer)] = bitcast<u32>(0.0);
}
```

**Step 3: Build**

Run: `cargo build`

**Step 4: Commit**

```bash
git add src/gpu/beam_write.rs src/gpu/beam_write.wgsl
git commit -m "feat: tier-aware beam write with instant energy boost"
```

---

### Task 9: Update spectral_resolve to sum multi-tier contributions

**Files:**

- Modify: `src/gpu/spectral_resolve.rs` (SpectralResolveParams)
- Modify: `src/gpu/spectral_resolve.wgsl`

**Step 1: Add tier info to SpectralResolveParams**

```rust
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct SpectralResolveParams {
    cie_x: [[f32; 4]; 4],
    cie_y: [[f32; 4]; 4],
    cie_z: [[f32; 4]; 4],
    pub slow_exp_count: u32,
    pub has_power_law: u32,
    pub power_law_alpha: f32,
    pub power_law_beta: f32,
}
```

Update the constructor:

```rust
impl SpectralResolveParams {
    pub fn new() -> Self {
        let mut cie_x = [[0.0f32; 4]; 4];
        let mut cie_y = [[0.0f32; 4]; 4];
        let mut cie_z = [[0.0f32; 4]; 4];

        for (i, &(x, y, z)) in CIE_INTEGRATION_WEIGHTS.iter().enumerate() {
            cie_x[i / 4][i % 4] = x;
            cie_y[i / 4][i % 4] = y;
            cie_z[i / 4][i % 4] = z;
        }

        Self {
            cie_x,
            cie_y,
            cie_z,
            slow_exp_count: 2, // default for P1
            has_power_law: 0,
            power_law_alpha: 0.0,
            power_law_beta: 0.0,
        }
    }

    pub fn update_from_phosphor(
        &mut self,
        terms: &[phosphor_data::DecayTerm],
        tau_cutoff: f32,
    ) {
        let class = phosphor_data::classify_decay_terms(terms, tau_cutoff);
        self.slow_exp_count = class.slow_exp_count as u32;
        self.has_power_law = if class.has_power_law { 1 } else { 0 };

        // Extract power-law params if present
        for term in terms {
            if let phosphor_data::DecayTerm::PowerLaw { alpha, beta, .. } = term {
                self.power_law_alpha = *alpha;
                self.power_law_beta = *beta;
                break;
            }
        }
    }
}
```

**Step 2: Update spectral_resolve.wgsl**

```wgsl
struct SpectralResolveParams {
    cie_x: array<vec4<f32>, 4>,
    cie_y: array<vec4<f32>, 4>,
    cie_z: array<vec4<f32>, 4>,
    slow_exp_count: u32,
    has_power_law: u32,
    power_law_alpha: f32,
    power_law_beta: f32,
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let coord = vec2<i32>(in.position.xy);

    var X = 0.0;
    var Y = 0.0;
    var Z = 0.0;

    for (var band = 0u; band < SPECTRAL_BANDS; band++) {
        var energy = 0.0;

        // Tier 2: sum slow exponential layers for this band
        for (var term = 0u; term < params.slow_exp_count; term++) {
            energy += load_accum(coord.x, coord.y,
                term * SPECTRAL_BANDS + band);
        }

        // Tier 3: power-law from stored peak and elapsed time
        if params.has_power_law == 1u {
            let pl_base = params.slow_exp_count * SPECTRAL_BANDS;
            let peak = load_accum(coord.x, coord.y, pl_base + band);
            if peak > 0.0 {
                let time_layer = pl_base + SPECTRAL_BANDS;
                let elapsed = load_accum(coord.x, coord.y, time_layer);
                energy += peak * pow(
                    params.power_law_alpha / (elapsed + params.power_law_alpha),
                    params.power_law_beta);
            }
        }

        X += energy * get_cie_weight(0u, band);
        Y += energy * get_cie_weight(1u, band);
        Z += energy * get_cie_weight(2u, band);
    }

    // XYZ -> linear sRGB (IEC 61966-2-1)
    var rgb = vec3<f32>(
         3.2406 * X - 1.5372 * Y - 0.4986 * Z,
        -0.9689 * X + 1.8758 * Y + 0.0415 * Z,
         0.0557 * X - 0.2040 * Y + 1.0570 * Z,
    );

    rgb = gamut_map(rgb, Y);
    return vec4<f32>(rgb, Y);
}
```

**Step 3: Build and run**

Run: `cargo build`

**Step 4: Commit**

```bash
git add src/gpu/spectral_resolve.rs src/gpu/spectral_resolve.wgsl
git commit -m "feat: spectral resolve sums multi-tier decay contributions"
```

---

### Task 10: Wire up GPU pipeline -- connect new params through render loop

**Files:**

- Modify: `src/gpu/mod.rs` (GpuState fields, render method)
- Modify: `src/main.rs` (parameter passing)
- Modify: `src/ui/engineer_panel.rs` (EngineerState)
- Modify: `src/ui/mod.rs` (phosphor change handling)

**Step 1: Update EngineerState**

Remove `tau_fast`, `tau_slow`, `a_fast` from `EngineerState`. Replace `sync_from_phosphor`:

```rust
pub struct EngineerState {
    // Beam
    pub sigma_core: f32,
    pub sigma_halo: f32,
    pub halo_fraction: f32,
    pub space_charge: f32,
    pub accel_voltage: f32,
    // Faceplate scatter
    pub scatter_threshold: f32,
    pub scatter_sigma: f32,
    pub scatter_intensity: f32,
    // Composite / display
    pub tonemap_mode: TonemapMode,
    pub exposure: f32,
    pub white_point: f32,
    pub glass_tint: [f32; 3],
    pub curvature: f32,
    pub edge_falloff: f32,
    // Resolution
    pub accum_resolution_scale: f32,
}
```

No more `sync_from_phosphor` for decay params -- that's handled directly by the GPU state.

**Step 2: Update GpuState**

Add `pub fn switch_phosphor(&mut self, phosphor: &PhosphorType)` method that rebuilds DecayParams, EmissionParams, SpectralResolveParams, and resizes AccumulationBuffer if needed.

**Step 3: Update main.rs render loop**

Replace `gpu.decay_params = DecayParams::new(eng.tau_fast, eng.tau_slow)` with:

```rust
// Decay and emission params are set by switch_phosphor(),
// only dt needs updating per frame
```

Add phosphor-change detection:

```rust
if phosphor_changed {
    gpu.switch_phosphor(phosphor);
}
```

**Step 4: Build and test visually**

Run: `cargo run --release`
Expected: The CRT displays with correct decay behavior. P1 should show persistent green traces. P31 should show a characteristic long afterglow tail from the power-law component.

**Step 5: Commit**

```bash
git add src/gpu/mod.rs src/main.rs src/ui/engineer_panel.rs src/ui/mod.rs
git commit -m "feat: wire three-tier decay through GPU pipeline and render loop"
```

---

### Task 11: Handle phosphor switching (accumulation buffer resize)

**Files:**

- Modify: `src/gpu/mod.rs`
- Modify: `src/main.rs`

When the user switches phosphors, the accumulation buffer layer count may change. Need to reallocate the buffer and zero it.

**Step 1: Implement switch_phosphor on GpuState**

```rust
pub fn switch_phosphor(&mut self, phosphor: &PhosphorType) {
    let terms = &phosphor.fluorescence.decay_terms;
    let class = phosphor_data::classify_decay_terms(terms, TAU_CUTOFF);
    let layers = (class.accum_layers() as u32).max(1);

    if layers != self.accum.layers {
        self.accum = AccumulationBuffer::new(
            &self.device, self.accum.resolution, layers);
    } else {
        // Zero the buffer even if same size -- old phosphor's data is invalid
        self.queue.write_buffer(
            &self.accum.buffer, 0,
            &vec![0u8; (self.accum.buffer.size()) as usize]);
    }

    self.decay_params = DecayParams::from_terms(terms, TAU_CUTOFF);
    self.emission_params = EmissionParams::from_phosphor(
        &phosphor.fluorescence.emission_weights, terms, TAU_CUTOFF);
    self.spectral_resolve_params.update_from_phosphor(terms, TAU_CUTOFF);
}
```

**Step 2: Call from main.rs on phosphor change**

In `handle_viewport_event`, detect phosphor change (compare index) and call `gpu.switch_phosphor()`.

**Step 3: Test**

Run: `cargo run --release`
Switch between P1 (exponential-only) and P31 (power-law). Verify no crashes and the buffer resizes correctly.

**Step 4: Commit**

```bash
git add src/gpu/mod.rs src/main.rs
git commit -m "feat: reallocate accumulation buffer on phosphor switch"
```

---

### Task 12: Update engineer panel UI for new decay model

**Files:**

- Modify: `src/ui/engineer_panel.rs`

**Step 1: Replace decay sliders with term display**

Remove the `tau_fast`/`tau_slow`/`a_fast` sliders. Show a read-only summary of the current phosphor's decay terms with their tier classification:

```rust
ui.heading("Decay");

let terms = &phosphor.fluorescence.decay_terms;
let class = phosphor_data::classify_decay_terms(terms, crate::gpu::TAU_CUTOFF);

ui.label(format!("{} terms: {} instant, {} slow, {}",
    terms.len(),
    class.instant_exp_count,
    class.slow_exp_count,
    if class.has_power_law { "power-law" } else { "no power-law" },
));
ui.label(format!("Buffer layers: {}", class.accum_layers()));

for term in terms {
    match term {
        phosphor_data::DecayTerm::Exponential { amplitude, tau } => {
            let tier = if *tau < crate::gpu::TAU_CUTOFF { "T1" } else { "T2" };
            ui.label(format!("  [{tier}] exp: A={amplitude:.3e}, tau={}",
                format_time(*tau)));
        }
        phosphor_data::DecayTerm::PowerLaw { amplitude, alpha, beta } => {
            ui.label(format!("  [T3] pow: A={amplitude:.3e}, alpha={}, beta={beta:.2}",
                format_time(*alpha)));
        }
    }
}
```

Add helper:

```rust
fn format_time(seconds: f32) -> String {
    if seconds >= 1.0 {
        format!("{seconds:.3} s")
    } else if seconds >= 1e-3 {
        format!("{:.3} ms", seconds * 1e3)
    } else if seconds >= 1e-6 {
        format!("{:.3} us", seconds * 1e6)
    } else {
        format!("{:.1} ns", seconds * 1e9)
    }
}
```

**Step 2: Build and run**

Run: `cargo run --release`
Verify the engineer panel shows the correct terms for each phosphor.

**Step 3: Commit**

```bash
git add src/ui/engineer_panel.rs
git commit -m "feat: engineer panel shows classified decay terms per phosphor"
```

---

### Task 13: Integration test -- verify decay curves

**Files:**

- Create: `tests/decay_curve.rs` (integration test)

**Step 1: Write tests**

```rust
use phosphor_data::{DecayTerm, classify_decay_terms};

#[test]
fn p1_decay_matches_selomulya() {
    let terms = vec![
        DecayTerm::Exponential { amplitude: 6.72, tau: 0.00288 },
        DecayTerm::Exponential { amplitude: 1.0, tau: 0.0151 },
    ];

    let sum_a: f32 = terms.iter().map(|t| match t {
        DecayTerm::Exponential { amplitude, .. } => *amplitude,
        _ => 0.0,
    }).sum();

    let at = |t: f32| -> f32 {
        terms.iter().map(|term| match term {
            DecayTerm::Exponential { amplitude, tau } =>
                (amplitude / sum_a) * (-t / tau).exp(),
            _ => 0.0,
        }).sum::<f32>()
    };

    // At t=0, normalized intensity = 1.0
    assert!((at(0.0) - 1.0).abs() < 0.01);

    // Selomulya: average lifetime 8.233 ms
    let at_8ms = at(0.008);
    assert!(at_8ms > 0.2 && at_8ms < 0.5, "at 8ms: {at_8ms}");
}

#[test]
fn p31_classification_correct() {
    let terms = vec![
        DecayTerm::PowerLaw { amplitude: 2.1e-4, alpha: 5.5e-6, beta: 1.1 },
        DecayTerm::Exponential { amplitude: 90.0, tau: 31.8e-9 },
        DecayTerm::Exponential { amplitude: 100.0, tau: 227e-9 },
        DecayTerm::Exponential { amplitude: 37.0, tau: 1.06e-6 },
    ];
    let class = classify_decay_terms(&terms, 1e-4);
    assert_eq!(class.instant_exp_count, 3);
    assert_eq!(class.slow_exp_count, 0);
    assert!(class.has_power_law);
    assert_eq!(class.accum_layers(), 17);
}

#[test]
fn p31_power_law_long_tail() {
    // ZnS:Cu power-law: I(t) = A / (t + alpha)^beta
    let alpha: f32 = 5.5e-6;
    let beta: f32 = 1.1;

    // At t = 1ms (~180x alpha): still significant
    let at_1ms = (alpha / (0.001 + alpha)).powf(beta);
    assert!(at_1ms > 1e-4, "at 1ms: {at_1ms}");

    // At t = 1s: very small but nonzero
    let at_1s = (alpha / (1.0 + alpha)).powf(beta);
    assert!(at_1s < 1e-6, "at 1s: {at_1s}");
    assert!(at_1s > 0.0, "must be nonzero");
}
```

**Step 2: Run**

Run: `cargo test --test decay_curve`

**Step 3: Commit**

```bash
git add tests/decay_curve.rs
git commit -m "test: integration tests for decay curves and tier classification"
```

---

## Execution Order

Tasks 1-5 form the data model chain (sequential):

```
1 -> 2 -> 3 -> 4 -> 5
```

Tasks 6-9 form the GPU pipeline chain (sequential, depends on 5):

```
5 -> 6 -> 7 -> 8 -> 9
```

Task 10 wires everything together (depends on 9).
Task 11 adds phosphor switching (depends on 10).
Task 12 updates the UI (depends on 10).
Task 13 is independent (can run after 2).

**Critical path:** 1 -> 2 -> 3 -> 4 -> 5 -> 6 -> 7 -> 8 -> 9 -> 10 -> 11 -> 12

**Parallel after Task 2:** Task 13 (integration tests)
