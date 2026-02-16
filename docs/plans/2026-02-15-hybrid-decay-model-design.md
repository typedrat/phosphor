# Hybrid Phosphor Decay Model

Replaces the uniform bi-exponential decay with a physically-accurate three-tier
model based on Kuhn (2002) PMT measurements and Dyall (1948) kinetic
classifications.

## Motivation

The current model applies `I(t) = a_fast * exp(-t/τ_fast) + a_slow * exp(-t/τ_slow)`
to all phosphors, with parameters fitted from Tektronix 10%/1%/0.1% decay time
data. This is physically wrong for ZnS-based phosphors (P2, P7, P11, P14, P17,
P31) which exhibit **inverse power-law decay** from bimolecular donor-acceptor
pair recombination. Only Mn²⁺-activated silicates (P1, P3) are legitimately
bi-exponential.

### Decay kinetics by phosphor class

| Class           | Phosphors    | Mechanism                               | Functional form          |
| --------------- | ------------ | --------------------------------------- | ------------------------ |
| Silicate (Mn²⁺) | P1, P3       | Two Mn²⁺ crystallographic sites         | Bi-exponential           |
| ZnS:Ag          | P11, P22B    | Bimolecular DAP recombination           | Power-law + exponentials |
| ZnS:Cu          | P31, P22G    | Bimolecular DAP recombination           | Power-law + exponentials |
| ZnS:Cu,Ag       | P2           | Mixed                                   | Inverse power-law        |
| Y₂O₂S:Eu        | P22R         | Multi-site exponential                  | Sum of 6 exponentials    |
| ZnO:Zn          | P15, P24     | Near-exponential                        | Fast single exponential  |
| Cascade         | P7, P14, P17 | Dual-layer: fast flash + slow afterglow | Two-component            |

### Key data sources

- **Kuhn (2002)**: 5 GHz PMT impulse response of P22 RGB, fitted as hybrid
  power-law + exponential models.
- **Selomulya et al. (2003)**: Mn²⁺ thin-film decay for P1: τ₁ = 2.88 ms,
  τ₂ = 15.1 ms, A₁/A₂ = 6.72.
- **PhysicsOpenLab (2019)**: Experimental confirmation that ZnS:Ag and ZnS:Cu
  follow second-order (power-law) kinetics.

## Three-Tier Architecture

### Tier 1: Instantaneous emission (τ << frame dt)

Exponential terms with τ below a cutoff (~100 µs) are evaluated analytically
during beam write. The time-integrated emission is `∫₀^∞ A·exp(-t/τ) dt = A·τ`.
This contributes directly to a transient HDR buffer as instantaneous brightness.
No persistent buffer state needed.

**Rationale:** At 60 fps (dt ≈ 16.6 ms), terms like P22 Green's 31.8 ns, 227 ns,
and 1.06 µs exponentials decay to zero within a single frame. Storing them in the
accumulation buffer wastes memory and compute.

### Tier 2: Slow exponential (multiplicative, existing approach)

Exponential terms with τ ≥ cutoff go into accumulation buffer layers, decayed
each frame via `value *= exp(-dt/τ)`. Exact and cheap. One set of 16 spectral-band
layers per slow-exponential term.

**Example:** P1's two terms (τ₁ = 2.88 ms, τ₂ = 15.1 ms) both exceed the cutoff.
P22 Red's 99.5 µs and 442 µs terms also qualify. Borderline terms (6.37 µs,
19.9 µs) go into the accumulation buffer for safety.

### Tier 3: Power-law (elapsed-time tracking)

Power-law terms `A / (t + α)^β` use:

- **16 spectral-band layers** for peak energy (deposited by beam write)
- **1 shared elapsed-time layer** (incremented by dt each frame, reset to 0 on
  new beam hit)
- **Threshold cutoff:** when `peak / (elapsed + α)^β < threshold` (e.g. 1e-6 of
  original peak), zero the peak energy and skip evaluation. Saves GPU cycles on
  dead texels.
- **Energy folding on revisit:** when a new beam hit arrives at a texel with
  existing power-law state, the old contribution is negligible (at realistic
  revisit intervals t/α ≈ 10³–10⁴, the old power-law value is ~10⁻⁴ of peak).
  The new hit simply resets elapsed time to 0 and sets the peak energy.

### Buffer layout per phosphor

| Phosphor          | Tier 1 (instant) | Tier 2 layers | Tier 3 layers | Total |
| ----------------- | ---------------- | ------------- | ------------- | ----- |
| P1                | —                | 16 × 2 = 32   | —             | 32    |
| P31 (≈Kuhn Green) | 3 fast exp       | —             | 16 + 1 = 17   | 17    |
| P11 (≈Kuhn Blue)  | 3 fast exp       | —             | 16 + 1 = 17   | 17    |
| P22 Red           | 2 fast exp       | 16 × 4 = 64   | —             | 64    |

## Data Model

### Rust types

```rust
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(tag = "type")]
pub enum DecayTerm {
    #[serde(rename = "exponential")]
    Exponential { amplitude: f32, tau: f32 },
    #[serde(rename = "power_law")]
    PowerLaw { amplitude: f32, alpha: f32, beta: f32 },
}

#[derive(Debug, Clone, PartialEq)]
pub struct PhosphorLayer {
    pub emission_weights: [f32; SPECTRAL_BANDS],
    pub decay_terms: Vec<DecayTerm>,  // max 8 terms
}
```

### TOML format

The `t_10pct`/`t_1pct`/`t_01pct` fields and Levenberg-Marquardt fitting are
removed entirely. Each phosphor defines explicit decay terms:

```toml
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

[[P31.decay_terms]]
type = "exponential"
amplitude = 100.0
tau = 227.0e-9

[[P31.decay_terms]]
type = "exponential"
amplitude = 37.0
tau = 1.06e-6
```

For dual-layer phosphors, each sub-table gets its own decay_terms array:

```toml
[P7]
dual_layer = true
# ...

[[P7.fluorescence.decay_terms]]
type = "power_law"
amplitude = ...

[[P7.phosphorescence.decay_terms]]
type = "power_law"
amplitude = ...
```

## GPU Uniform Structures

### Decay term packing (max 8 terms)

```wgsl
struct DecayTermGpu {
    amplitude: f32,
    param1: f32,    // tau (exponential) or alpha (power_law)
    param2: f32,    // 0.0 (exponential) or beta (power_law)
    type_flag: f32, // 0.0 = exponential, 1.0 = power_law
}
```

8 terms × 16 bytes = 128 bytes per layer.

### Decay params uniform

```wgsl
struct DecayParams {
    dt: f32,
    threshold: f32,
    tau_cutoff: f32,     // tier 1/2 boundary
    term_count: u32,
    terms: array<DecayTermGpu, 8>,
    // Counts for tier classification:
    instant_count: u32,  // terms with tau < tau_cutoff
    slow_exp_count: u32, // exponential terms with tau >= tau_cutoff
    has_power_law: u32,  // 1 if any power_law term exists, 0 otherwise
    _pad: u32,
}
```

### Accumulation buffer layout

The accumulation buffer becomes variable-size per phosphor:

- Base: `slow_exp_count × SPECTRAL_BANDS` layers (tier 2)
- If `has_power_law`: `+ SPECTRAL_BANDS + 1` layers (tier 3: peak energy + elapsed time)

The `AccumulationBuffer::new()` constructor takes the classified term counts
instead of a fixed `phosphor_layers` argument.

## Shader Changes

### beam_write.wgsl

New tier-1 logic: for each instant exponential term (τ < cutoff), compute
`A × τ × emission_weight × intensity × profile` and accumulate into a transient
contribution. This is written to the HDR buffer (or a separate instantaneous
buffer) before the spectral resolve pass.

For tier-2 and tier-3 terms, the beam write deposits energy into the
accumulation buffer as before, but now indexes into the correct layer set
for each term. For tier-3 power-law layers, it also resets the elapsed-time
layer to 0.0.

### decay.wgsl

Becomes tier-aware:

```wgsl
// Tier 2: slow exponentials
for (var term = 0u; term < params.slow_exp_count; term++) {
    let tau = params.terms[params.instant_count + term].param1;
    let factor = exp(-params.dt / tau);
    for (var band = 0u; band < SPECTRAL_BANDS; band++) {
        let layer = term * SPECTRAL_BANDS + band;
        let val = load_accum(coord, layer);
        store_accum(coord, layer, decay_value(val, factor, threshold));
    }
}

// Tier 3: power-law
if (params.has_power_law == 1u) {
    let base = params.slow_exp_count * SPECTRAL_BANDS;
    let time_layer = base + SPECTRAL_BANDS;
    var elapsed = load_accum(coord, time_layer);
    elapsed += params.dt;
    store_accum(coord, time_layer, elapsed);

    // Find the power-law term
    for (var i = 0u; i < params.term_count; i++) {
        if (params.terms[i].type_flag == 1.0) {
            let alpha = params.terms[i].param1;
            let beta = params.terms[i].param2;
            for (var band = 0u; band < SPECTRAL_BANDS; band++) {
                let peak = load_accum(coord, base + band);
                if (peak > 0.0) {
                    let value = peak * params.terms[i].amplitude
                              * pow(alpha / (elapsed + alpha), beta);
                    if (value < threshold) {
                        store_accum(coord, base + band, 0.0);
                    }
                }
            }
            break; // only one power-law term
        }
    }
}
```

### spectral_resolve.wgsl

Sums contributions from all tiers:

- Tier 2: sum across slow-exponential layers per band (weighted by term amplitude,
  already baked into the deposited energy)
- Tier 3: evaluate `peak * A / (elapsed + α)^β` per band
- Add to the HDR buffer (which may already contain tier-1 instantaneous contributions)

## What Changes

### Removed

- `fit_decay()` function and `levenberg-marquardt`/`nalgebra` dependencies
- `t_10pct`/`t_1pct`/`t_01pct` fields from TOML and `PhosphorData`
- Fixed `tau_fast`/`tau_slow`/`a_fast`/`a_slow` fields from `PhosphorLayer`
- `DecayFitProblem` struct

### Changed

- `PhosphorLayer`: gains `decay_terms: Vec<DecayTerm>`
- `DecayParams`: variable term count, tier classification
- `decay.wgsl`: three-tier dispatch
- `beam_write.wgsl`: tier-1 instantaneous emission
- `spectral_resolve.wgsl`: multi-tier summation
- `phosphors.toml`: explicit decay terms per phosphor
- `EngineerState`: decay controls adapt to variable terms
- `AccumulationBuffer`: variable layer count based on classified terms
- `phosphor_data_macro`: format new struct layout

### Unchanged

- Spectral band system (16 bands, CIE integration weights)
- CIE → XYZ → sRGB conversion
- Composite, faceplate scatter, bloom passes
- Beam spot profile (Gaussian + halo)
- Input sources (oscilloscope, audio, vector, external)
- Window management, UI layout

## Phosphor Data Values

### P1 (Zn₂SiO₄:Mn) — Bi-exponential

Source: Selomulya et al. (2003)

| Term | Type        | Amplitude | τ       |
| ---- | ----------- | --------- | ------- |
| 1    | exponential | 6.72      | 2.88 ms |
| 2    | exponential | 1.0       | 15.1 ms |

### P31 ≈ P22 Green (ZnS:Cu,Al) — Power-law + 3 exponentials

Source: Kuhn (2002) P22 Green model

| Term | Type        | Amplitude | τ / α      | β   |
| ---- | ----------- | --------- | ---------- | --- |
| 1    | power_law   | 2.1e-4    | α = 5.5 µs | 1.1 |
| 2    | exponential | 90.0      | 31.8 ns    | —   |
| 3    | exponential | 100.0     | 227 ns     | —   |
| 4    | exponential | 37.0      | 1.06 µs    | —   |

### P11 ≈ P22 Blue (ZnS:Ag) — Power-law + 3 exponentials

Source: Kuhn (2002) P22 Blue model

| Term | Type        | Amplitude | τ / α      | β    |
| ---- | ----------- | --------- | ---------- | ---- |
| 1    | power_law   | 1.9e-4    | α = 5.0 µs | 1.11 |
| 2    | exponential | 1100.0    | 39.8 ns    | —    |
| 3    | exponential | 1000.0    | 145 ns     | —    |
| 4    | exponential | 75.0      | 1.59 µs    | —    |

### P3, P20, P32, P4 — Estimated from Tektronix data

These phosphors lack Kuhn-quality measurements. We estimate bi-exponential
parameters from the Tektronix 10%/1%/0.1% data using the existing LM fitter
as a one-time conversion, then store the resulting explicit terms.

### P2, P7, P14, P17 — Dual-layer with power-law tails

Fluorescence and phosphorescence layers each get their own decay_terms.
The fast fluorescence flash uses exponential terms; the slow phosphorescence
uses power-law terms with β estimated from Dyall (1948) persistence curves.
Exact Kuhn-quality data is not available for these; we use the Tektronix
timing data to constrain the power-law parameters.

### P15, P24 — Very fast, near-exponential

Single exponential term, τ ≈ 3 µs (P15) / 1.5 µs (P24). These are entirely
tier-1 instantaneous at 60 fps.
