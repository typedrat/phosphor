# Phosphor Model

## Phosphor Type Database

Each JEDEC-registered phosphor type (P1, P2, P7, P11, P31, etc.) is defined as a Rust struct containing all physical parameters needed for simulation. The database is compiled into the binary from the Tektronix CRT Data reference tables in `docs/`.

### Per-Layer Data

Each phosphor has 1-2 layers. Single-layer phosphors (P1, P11, P31) have identical fluorescence and phosphorescence colors. Dual-layer phosphors (P2, P7, P14, marked with `*` in the reference tables) have physically separate layers with distinct emission spectra and decay characteristics.

```rust
struct PhosphorLayer {
    /// Spectral emission weights across SPECTRAL_BANDS.
    /// Normalized so the sum = 1.0. Derived from the phosphor compound's
    /// emission spectrum, sampled and integrated into each band.
    emission_weights: [f32; SPECTRAL_BANDS],

    /// Two-term exponential decay: I(t) = a_fast * exp(-t/tau_fast) + a_slow * exp(-t/tau_slow)
    /// Fitted to match the Tektronix decay data (time to 10%, 1%, 0.1% of initial).
    tau_fast: f32,       // seconds
    tau_slow: f32,       // seconds
    a_fast: f32,         // amplitude weight of fast component (a_fast + a_slow = 1.0)
    a_slow: f32,         // amplitude weight of slow component
}
```

### Per-Phosphor Data

```rust
struct PhosphorType {
    designation: &'static str,     // "P1", "P7", etc.
    category: PhosphorCategory,    // GeneralPurpose, ShortDecay, VideoDisplay, LongDecay, etc.
    is_dual_layer: bool,

    fluorescence: PhosphorLayer,   // always present — the fast initial emission
    phosphorescence: PhosphorLayer, // for single-layer, same emission weights but separate decay

    peak_wavelength_nm: f32,       // dominant emission wavelength
    relative_luminance: f32,       // from reference table (P31 = 100%)
    relative_writing_speed: f32,   // from reference table (P11 = 100%)
}
```

## Spectral Emission Curves

The emission weights per band are derived from the known phosphor compounds:

| Phosphor | Compound                                           | Peak λ | Emission Character                        |
| -------- | -------------------------------------------------- | ------ | ----------------------------------------- |
| P1       | ZnSiO₄:Mn (willemite)                              | 520nm  | Broad green, ~40nm FWHM                   |
| P2       | ZnS:Cu (fluorescence) + ZnCdS:Cu (phosphorescence) | 510nm  | Blue-green flash → yellow-green afterglow |
| P7       | ZnS:Ag (fluorescence) + ZnCdS:Cu (phosphorescence) | 450nm  | Blue-white flash → yellow-green afterglow |
| P11      | ZnS:Ag                                             | 450nm  | Narrow violet-blue, ~25nm FWHM            |
| P31      | ZnS:Cu                                             | 530nm  | Broad green, ~50nm FWHM                   |

For each compound, we approximate the emission as a Gaussian centered on the peak wavelength with the appropriate FWHM, then integrate into `SPECTRAL_BANDS` bins. For dual-layer phosphors, the two layers have independent Gaussian profiles at different peaks.

Where published CIE spectral data is available for a compound, we use that directly. Otherwise, the Gaussian approximation is sufficient — most phosphor emissions are smooth, broad peaks.

## Decay Fitting

The Tektronix Phosphor Decay Table (Rev A, 1966) provides three data points per phosphor:

- Time to decay to 10% of initial intensity
- Time to decay to 1%
- Time to decay to 0.1%

We fit a two-term exponential `I(t) = A₁·exp(-t/τ₁) + A₂·exp(-t/τ₂)` where `A₁ + A₂ = 1` to these three points using least-squares. This captures:

- The fast initial drop (dominated by `τ₁`)
- The slow phosphorescent tail (dominated by `τ₂`)

### Reference Decay Data (from Tektronix tables)

**General Purpose:**
| Type | 10% | 1% | 0.1% |
|------|-----|----|------|
| P1 | 27ms | 60ms | 95ms |
| P20 | 230µs | 2300µs | 11,000µs |
| P31 | 340µs | 4500µs | 32,000µs |

**Short Decay:**
| Type | 10% | 1% | 0.1% |
|------|-----|----|------|
| P15 | 2µs | 12µs | 50µs |
| P16 | 1.1µs | 75µs | — |
| P24 | 18µs | 137µs | 590µs |

**Video Display:**
| Type | 10% | 1% | 0.1% |
|------|-----|----|------|
| P3 | 19ms | 45ms | 74ms |
| P4† | 320µs | 3300µs | 20,000µs |
| P6† | 330µs | 3300µs | 18,000µs |

**Long Decay — Sulfide:**
| Type | 10% | 1% | 0.1% |
|------|-----|----|------|
| P2 | 370µs | 4700µs | 51,000µs |
| P7* | 305µs | 5700µs | 66,000µs |
| P14* | 440µs | 6700µs | 55,000µs |
| P17\* | 130µs | 8700µs | 82,500µs |

`*` = double-layer screen. `†` = mixed phosphor.

## Dual-Layer Behavior

For dual-layer phosphors (P2, P7, P14, etc.), the two layers are physically distinct coatings on the CRT faceplate:

1. **Front layer** (facing the electron gun): Fast-decay fluorescent layer. Emits immediately when struck by electrons, decays quickly. Typically a blue or blue-white emitter (ZnS:Ag for P7).

2. **Back layer** (on the glass): Slow-decay phosphorescent layer. Excited by UV/light from the front layer, not directly by electrons. Decays slowly. Typically a yellow-green emitter (ZnCdS:Cu for P7).

In our model, both layers receive energy simultaneously when the beam hits (the front layer excites the back layer essentially instantaneously). Each layer then decays according to its own time constants and emits its own spectral profile. The visible result is the sum of both layers' emissions at any given time — producing the characteristic color shift from blue-white to yellow-green as P7 decays.
