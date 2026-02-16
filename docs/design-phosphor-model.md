# Phosphor Model

## Phosphor Type Database

Each JEDEC-registered phosphor type (P1, P2, P7, P11, P31, etc.) is defined as a Rust struct containing all physical parameters needed for simulation. The database is defined in `data/phosphors.toml` and baked into the binary at compile time via a proc macro.

### Per-Layer Data

Each phosphor has 1–2 layers. Single-layer phosphors (P1, P11, P31) have identical fluorescence and phosphorescence colors. Dual-layer phosphors (P2, P7, P14) have physically separate layers with distinct emission spectra and decay characteristics.

```rust
pub enum DecayTerm {
    Exponential { amplitude: f32, tau: f32 },
    PowerLaw { amplitude: f32, alpha: f32, beta: f32 },
}

pub struct PhosphorLayer {
    pub emission_weights: [f32; SPECTRAL_BANDS],
    pub decay_terms: Vec<DecayTerm>,  // max 8 terms
}
```

### Per-Phosphor Data

```rust
pub struct PhosphorType {
    pub designation: String,          // "P1", "P7", etc.
    pub description: String,
    pub category: PhosphorCategory,
    pub is_dual_layer: bool,

    pub fluorescence: PhosphorLayer,
    pub phosphorescence: PhosphorLayer,

    pub peak_wavelength_nm: f32,
    pub relative_luminance: f32,
    pub relative_writing_speed: f32,
}
```

## Spectral Emission Curves

The emission weights per band are derived from the known phosphor compounds, approximated as Gaussians centered on the peak wavelength with the appropriate FWHM, then integrated and normalized across `SPECTRAL_BANDS` bins:

| Phosphor | Compound                      | Peak λ | FWHM  | Emission Character                        |
| -------- | ----------------------------- | ------ | ----- | ----------------------------------------- |
| P1       | ZnSiO₄:Mn (willemite)         | 520nm  | ~40nm | Broad green                               |
| P2       | ZnS:Cu (fl.) + ZnCdS:Cu (ph.) | 510nm  | —     | Blue-green flash → yellow-green afterglow |
| P7       | ZnS:Ag (fl.) + ZnCdS:Cu (ph.) | 450nm  | —     | Blue-white flash → yellow-green afterglow |
| P11      | ZnS:Ag                        | 450nm  | ~25nm | Narrow violet-blue                        |
| P31      | ZnS:Cu                        | 530nm  | ~50nm | Broad green                               |

## Three-Tier Hybrid Decay Model

The uniform bi-exponential model is physically wrong for ZnS-based phosphors, which exhibit **inverse power-law decay** from bimolecular donor-acceptor pair recombination. Only Mn²⁺-activated silicates (P1, P3) are legitimately bi-exponential.

### Decay Kinetics by Phosphor Class

| Class           | Phosphors    | Mechanism                               | Functional Form          |
| --------------- | ------------ | --------------------------------------- | ------------------------ |
| Silicate (Mn²⁺) | P1, P3       | Two Mn²⁺ crystallographic sites         | Bi-exponential           |
| ZnS:Ag          | P11          | Bimolecular DAP recombination           | Power-law + exponentials |
| ZnS:Cu          | P31          | Bimolecular DAP recombination           | Power-law + exponentials |
| ZnS:Cu,Ag       | P2           | Mixed                                   | Inverse power-law        |
| Y₂O₂S:Eu        | P22R         | Multi-site exponential                  | Sum of exponentials      |
| ZnO:Zn          | P15, P24     | Near-exponential                        | Fast single exponential  |
| Cascade         | P7, P14, P17 | Dual-layer: fast flash + slow afterglow | Two-component            |

### Key Data Sources

- **Kuhn (2002)**: 5 GHz PMT impulse response of P22 RGB, fitted as hybrid power-law + exponential models.
- **Selomulya et al. (2003)**: Mn²⁺ thin-film decay for P1: τ₁ = 2.88 ms, τ₂ = 15.1 ms, A₁/A₂ = 6.72.
- **Tektronix (1966)**: CRT Phosphor Data sheets — 10%, 1%, 0.1% decay times used to constrain power-law parameters where Kuhn-quality data is unavailable.

### Three Tiers

**Tier 1 — Instantaneous (τ < 100µs):** Exponential terms that decay to zero within a single frame at 60fps. Their time-integrated emission `∫₀^∞ A·exp(-t/τ) dt = A·τ` is computed analytically during beam write and deposited as a one-frame scalar. No persistent buffer state needed.

**Tier 2 — Slow Exponential (τ ≥ 100µs):** Multiplicative decay in the accumulation buffer via `value *= exp(-dt/τ)` per frame. Exact and cheap. One scalar layer per term.

**Tier 3 — Power-Law:** For ZnS-based phosphors: `I(t) = peak * (α/(t+α))^β`. Uses a scalar peak-energy layer and an elapsed-time layer. When `peak * (α/(elapsed+α))^β < threshold`, the texel is zeroed to save compute. On revisit, elapsed time resets and peak energy is deposited fresh.

### Example Phosphor Data

**P1 (Zn₂SiO₄:Mn) — Bi-exponential (Selomulya 2003):**

| Term | Type        | Amplitude | τ       |
| ---- | ----------- | --------- | ------- |
| 1    | exponential | 6.72      | 2.88 ms |
| 2    | exponential | 1.0       | 15.1 ms |

Both terms are tier 2 (slow). Buffer: 2 scalar layers.

**P31 ≈ P22 Green (ZnS:Cu,Al) — Power-law + 3 exponentials (Kuhn 2002):**

| Term | Type        | Amplitude | τ / α      | β   |
| ---- | ----------- | --------- | ---------- | --- |
| 1    | power_law   | 2.1e-4    | α = 5.5 µs | 1.1 |
| 2    | exponential | 90.0      | 31.8 ns    | —   |
| 3    | exponential | 100.0     | 227 ns     | —   |
| 4    | exponential | 37.0      | 1.06 µs    | —   |

Terms 2–4 are tier 1 (instantaneous). Term 1 is tier 3 (power-law). Buffer: 3 layers (peak + elapsed + instant).

## Dual-Layer Behavior

For dual-layer phosphors (P2, P7, P14, etc.), the two layers are physically distinct coatings on the CRT faceplate:

1. **Front layer** (facing the electron gun): Fast-decay fluorescent layer. Emits immediately when struck by electrons, decays quickly. Typically a blue or blue-white emitter (ZnS:Ag for P7).

2. **Back layer** (on the glass): Slow-decay phosphorescent layer. Excited by UV/light from the front layer. Decays slowly. Typically a yellow-green emitter (ZnCdS:Cu for P7).

Both layers receive energy simultaneously when the beam hits. Each layer then decays according to its own terms and emits its own spectral profile. The visible result is the sum of both layers' emissions — producing the characteristic color shift from blue-white to yellow-green as P7 decays.

In the GPU pipeline, dual-layer phosphors use two emission groups in `SpectralResolveParams`, each with its own spectral weights and layer indices.
