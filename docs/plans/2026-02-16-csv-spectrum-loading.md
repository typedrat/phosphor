# CSV Spectrum Loading Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Allow phosphors to load measured emission spectra from CSV files instead of using Gaussian approximations.

**Architecture:** Add `csv_to_emission_weights()` to the spectral module that parses CSV data and integrates it into 16 spectral bands via trapezoidal integration. The TOML phosphor definition gains an optional `spectrum_csv` field; when present, the CSV is read (at compile time for built-in phosphors, at runtime for user-added) and used instead of the Gaussian approximation.

**Tech Stack:** Rust, no new dependencies (hand-rolled CSV parsing for the simple format).

---

### Task 1: Add `csv_to_emission_weights` to spectral module

**Files:**

- Modify: `crates/phosphor-data/src/spectral.rs`

**Step 1: Write the failing test**

Add to the bottom of `crates/phosphor-data/src/spectral.rs` (the file currently has no tests):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_to_emission_weights_basic() {
        // Narrow spike at 525nm — should land in band containing 525nm
        let csv = "\
# comment
wavelength_nm,rel_intensity
500,0
520,0
525,100
530,0
560,0
";
        let weights = csv_to_emission_weights(csv).unwrap();
        assert_eq!(weights.len(), SPECTRAL_BANDS);

        // Weights must be normalized
        let sum: f32 = weights.iter().sum();
        assert!((sum - 1.0).abs() < 0.01, "sum was {sum}");

        // Peak band should contain 525nm
        let peak_band = weights
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .unwrap()
            .0;
        let (lo, hi) = band_range(peak_band);
        assert!(525.0 >= lo && 525.0 < hi, "peak band [{lo}, {hi}] doesn't contain 525nm");
    }

    #[test]
    fn csv_to_emission_weights_uniform() {
        // Flat spectrum across the visible range — all bands roughly equal
        let mut lines = vec!["wavelength_nm,rel_intensity".to_string()];
        for wl in (380..=780).step_by(10) {
            lines.push(format!("{wl},100"));
        }
        let csv = lines.join("\n");
        let weights = csv_to_emission_weights(&csv).unwrap();
        let expected = 1.0 / SPECTRAL_BANDS as f32;
        for (i, &w) in weights.iter().enumerate() {
            assert!(
                (w - expected).abs() < 0.05,
                "band {i}: expected ~{expected}, got {w}"
            );
        }
    }

    #[test]
    fn csv_to_emission_weights_extra_columns_ignored() {
        let csv = "\
wavelength_nm,wavelength_angstrom,energy_eV,rel_intensity
500,5000,2.480,0
525,5250,2.362,100
550,5500,2.255,0
";
        let weights = csv_to_emission_weights(csv).unwrap();
        let sum: f32 = weights.iter().sum();
        assert!((sum - 1.0).abs() < 0.01);
    }

    #[test]
    fn csv_to_emission_weights_missing_column_errors() {
        let csv = "wavelength_nm,something_else\n500,100\n";
        assert!(csv_to_emission_weights(csv).is_err());
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p phosphor-data csv_to_emission_weights`
Expected: FAIL — `csv_to_emission_weights` does not exist.

**Step 3: Implement `csv_to_emission_weights`**

Add to `crates/phosphor-data/src/spectral.rs`, above the `#[cfg(test)]` block:

```rust
use std::fmt;

#[derive(Debug)]
pub enum CsvSpectrumError {
    MissingColumn(&'static str),
    ParseFloat { line: usize, col: &'static str, err: std::num::ParseFloatError },
    TooFewPoints,
    ZeroIntensity,
}

impl fmt::Display for CsvSpectrumError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingColumn(col) => write!(f, "CSV missing required column: {col}"),
            Self::ParseFloat { line, col, err } => {
                write!(f, "line {line}, column '{col}': {err}")
            }
            Self::TooFewPoints => write!(f, "CSV must contain at least 2 data points"),
            Self::ZeroIntensity => write!(f, "CSV total intensity is zero"),
        }
    }
}

impl std::error::Error for CsvSpectrumError {}

/// Parse a CSV emission spectrum and integrate it into spectral band weights.
///
/// The CSV must have a header row with at least `wavelength_nm` and `rel_intensity`
/// columns. Lines starting with `#` are skipped. Additional columns are ignored.
/// Returns normalized weights (sum = 1.0) via trapezoidal integration per band.
pub fn csv_to_emission_weights(csv_text: &str) -> Result<[f32; SPECTRAL_BANDS], CsvSpectrumError> {
    let mut lines = csv_text.lines().filter(|l| !l.starts_with('#'));

    // Parse header
    let header = lines.next().ok_or(CsvSpectrumError::MissingColumn("wavelength_nm"))?;
    let columns: Vec<&str> = header.split(',').map(str::trim).collect();
    let wl_idx = columns.iter().position(|&c| c == "wavelength_nm")
        .ok_or(CsvSpectrumError::MissingColumn("wavelength_nm"))?;
    let int_idx = columns.iter().position(|&c| c == "rel_intensity")
        .ok_or(CsvSpectrumError::MissingColumn("rel_intensity"))?;

    // Parse data points
    let mut points: Vec<(f32, f32)> = Vec::new();
    for (line_num, line) in lines.enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split(',').map(str::trim).collect();
        let wl: f32 = fields[wl_idx].parse().map_err(|err| CsvSpectrumError::ParseFloat {
            line: line_num + 2, // +1 for header, +1 for 1-based
            col: "wavelength_nm",
            err,
        })?;
        let intensity: f32 = fields[int_idx].parse().map_err(|err| CsvSpectrumError::ParseFloat {
            line: line_num + 2,
            col: "rel_intensity",
            err,
        })?;
        points.push((wl, intensity));
    }

    if points.len() < 2 {
        return Err(CsvSpectrumError::TooFewPoints);
    }

    points.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());

    // Trapezoidal integration per band
    let mut weights = [0.0f32; SPECTRAL_BANDS];
    for band in 0..SPECTRAL_BANDS {
        let (band_lo, band_hi) = band_range(band);
        let mut integral = 0.0f32;

        for pair in points.windows(2) {
            let (x0, y0) = pair[0];
            let (x1, y1) = pair[1];

            // Skip segments entirely outside this band
            if x1 <= band_lo || x0 >= band_hi {
                continue;
            }

            // Clip segment to band boundaries with linear interpolation
            let (cx0, cy0) = if x0 < band_lo {
                let t = (band_lo - x0) / (x1 - x0);
                (band_lo, y0 + t * (y1 - y0))
            } else {
                (x0, y0)
            };
            let (cx1, cy1) = if x1 > band_hi {
                let t = (band_hi - x0) / (x1 - x0);
                (band_hi, y0 + t * (y1 - y0))
            } else {
                (x1, y1)
            };

            // Trapezoid area
            integral += (cy0 + cy1) * 0.5 * (cx1 - cx0);
        }

        weights[band] = integral;
    }

    // Normalize
    let sum: f32 = weights.iter().sum();
    if sum == 0.0 {
        return Err(CsvSpectrumError::ZeroIntensity);
    }
    for w in &mut weights {
        *w /= sum;
    }

    Ok(weights)
}
```

**Step 4: Run tests to verify they pass**

Run: `cargo test -p phosphor-data csv_to_emission_weights`
Expected: All 4 tests PASS.

**Step 5: Commit**

```bash
git add crates/phosphor-data/src/spectral.rs
git commit -m "feat: add csv_to_emission_weights for measured emission spectra

Parses CSV files with wavelength_nm and rel_intensity columns, integrates
via trapezoidal rule into 16 spectral bands, normalizes to sum=1.0.
Handles comment lines, extra columns, and non-uniform spacing."
```

---

### Task 2: Add `spectrum_csv` field to TOML deserialization

**Files:**

- Modify: `crates/phosphor-data/src/lib.rs`

**Step 1: Write the failing test**

Add to the `mod tests` block in `crates/phosphor-data/src/lib.rs`:

```rust
    #[test]
    fn spectrum_csv_field_parsed_from_toml() {
        let toml_str = r#"
[P1]
description = "Test with CSV."
category = "general_purpose"
peak_nm = 525.0
spectrum_csv = "spectra/test.csv"
relative_luminance = 50.0
relative_writing_speed = 60.0

[[P1.decay_terms]]
type = "exponential"
amplitude = 1.0
tau = 0.003
"#;
        let table: std::collections::BTreeMap<String, PhosphorData> =
            toml::from_str(toml_str).unwrap();
        let p1 = &table["P1"];
        assert_eq!(p1.spectrum_csv.as_deref(), Some("spectra/test.csv"));
    }

    #[test]
    fn spectrum_csv_field_absent_is_none() {
        let toml_str = r#"
[P1]
description = "Test without CSV."
category = "general_purpose"
peak_nm = 525.0
fwhm_nm = 38.0
relative_luminance = 50.0
relative_writing_speed = 60.0

[[P1.decay_terms]]
type = "exponential"
amplitude = 1.0
tau = 0.003
"#;
        let table: std::collections::BTreeMap<String, PhosphorData> =
            toml::from_str(toml_str).unwrap();
        let p1 = &table["P1"];
        assert!(p1.spectrum_csv.is_none());
    }
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p phosphor-data spectrum_csv_field`
Expected: FAIL — `PhosphorData` has no field `spectrum_csv`.

**Step 3: Add the field to `PhosphorData` and `LayerData`**

In `crates/phosphor-data/src/lib.rs`, add `spectrum_csv` to both structs:

```rust
#[derive(Debug, Deserialize)]
struct LayerData {
    peak_nm: f32,
    fwhm_nm: Option<f32>,          // was: f32
    #[serde(default)]
    spectrum_csv: Option<String>,   // NEW
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
    spectrum_csv: Option<String>,   // NEW
    #[serde(default)]
    decay_terms: Vec<DecayTerm>,
    relative_luminance: f32,
    relative_writing_speed: f32,
    fluorescence: Option<LayerData>,
    phosphorescence: Option<LayerData>,
}
```

Note: `LayerData.fwhm_nm` changes from `f32` to `Option<f32>` since it's no longer required when `spectrum_csv` is present.

**Step 4: Run tests to verify they pass**

Run: `cargo test -p phosphor-data spectrum_csv_field`
Expected: Both tests PASS.

Also run: `cargo test -p phosphor-data`
Expected: All existing tests still pass (the `fwhm_nm` change in `LayerData` is backwards-compatible since it was already gated behind `Option<LayerData>`).

**Step 5: Commit**

```bash
git add crates/phosphor-data/src/lib.rs
git commit -m "feat: add spectrum_csv field to phosphor TOML schema

Optional field on PhosphorData and LayerData. When present, emission
weights will be loaded from the CSV file instead of the Gaussian
approximation. LayerData.fwhm_nm becomes optional to match."
```

---

### Task 3: Wire `spectrum_csv` into `build_phosphor` with base path resolution

**Files:**

- Modify: `crates/phosphor-data/src/lib.rs`

**Step 1: Write the failing test**

Create a temporary CSV file in the test and use `load_phosphors_with_base_path`:

```rust
    #[test]
    fn build_phosphor_uses_csv_when_present() {
        // CSV with a narrow spike at 525nm
        let csv_content = "\
wavelength_nm,rel_intensity
500,0
520,0
525,100
530,0
560,0
";
        let dir = std::env::temp_dir().join("phosphor_test_csv");
        let _ = std::fs::create_dir_all(dir.join("spectra"));
        std::fs::write(dir.join("spectra/test.csv"), csv_content).unwrap();

        let toml_str = r#"
[TestCSV]
description = "Test with CSV spectrum."
category = "general_purpose"
peak_nm = 525.0
spectrum_csv = "spectra/test.csv"
relative_luminance = 50.0
relative_writing_speed = 60.0

[[TestCSV.decay_terms]]
type = "exponential"
amplitude = 1.0
tau = 0.003
"#;
        let phosphors = load_phosphors_with_base_path(toml_str, Some(&dir)).unwrap();
        let p = &phosphors[0];

        // Weights should be normalized
        let sum: f32 = p.fluorescence.emission_weights.iter().sum();
        assert!((sum - 1.0).abs() < 0.01, "sum was {sum}");

        // Peak band should contain 525nm
        let peak_band = p.fluorescence.emission_weights
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .unwrap()
            .0;
        let (lo, hi) = spectral::band_range(peak_band);
        assert!(525.0 >= lo && 525.0 < hi);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn build_phosphor_falls_back_to_gaussian_without_csv() {
        let toml_str = r#"
[TestGauss]
description = "Test without CSV."
category = "general_purpose"
peak_nm = 525.0
fwhm_nm = 38.0
relative_luminance = 50.0
relative_writing_speed = 60.0

[[TestGauss.decay_terms]]
type = "exponential"
amplitude = 1.0
tau = 0.003
"#;
        let phosphors = load_phosphors_with_base_path(toml_str, None).unwrap();
        let p = &phosphors[0];
        let expected = spectral::gaussian_emission_weights(525.0, 38.0);
        assert_eq!(p.fluorescence.emission_weights, expected);
    }
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p phosphor-data build_phosphor`
Expected: FAIL — `load_phosphors_with_base_path` does not exist.

**Step 3: Implement base-path-aware loading**

In `crates/phosphor-data/src/lib.rs`:

1. Change `build_phosphor` to accept a `base_path: Option<&Path>` parameter.
2. Add a helper `resolve_emission_weights` that chooses CSV vs Gaussian.
3. Add the new public API `load_phosphors_with_base_path`.
4. Make `load_phosphors` delegate to the new function with `None`.

```rust
use std::path::PathBuf;

fn resolve_emission_weights(
    peak_nm: f32,
    fwhm_nm: Option<f32>,
    spectrum_csv: Option<&str>,
    base_path: Option<&Path>,
    designation: &str,
) -> [f32; SPECTRAL_BANDS] {
    if let Some(csv_rel) = spectrum_csv {
        let base = base_path.unwrap_or_else(|| {
            panic!("{designation}: spectrum_csv requires a base path for resolution")
        });
        let csv_path = base.join(csv_rel);
        let csv_text = std::fs::read_to_string(&csv_path).unwrap_or_else(|e| {
            panic!("{designation}: failed to read {}: {e}", csv_path.display())
        });
        spectral::csv_to_emission_weights(&csv_text).unwrap_or_else(|e| {
            panic!("{designation}: failed to parse {}: {e}", csv_path.display())
        })
    } else {
        let fwhm = fwhm_nm.unwrap_or_else(|| {
            panic!("{designation}: need fwhm_nm or spectrum_csv for emission weights")
        });
        spectral::gaussian_emission_weights(peak_nm, fwhm)
    }
}

fn build_phosphor(designation: &str, data: &PhosphorData, base_path: Option<&Path>) -> PhosphorType {
    let make_layer =
        |peak: f32, fwhm: Option<f32>, csv: Option<&str>, terms: &[DecayTerm]| -> PhosphorLayer {
            PhosphorLayer {
                emission_weights: resolve_emission_weights(peak, fwhm, csv, base_path, designation),
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
        let fl_terms = if fl.decay_terms.is_empty() { &data.decay_terms } else { &fl.decay_terms };
        let ph_terms = if ph.decay_terms.is_empty() { &data.decay_terms } else { &ph.decay_terms };
        (
            make_layer(fl.peak_nm, fl.fwhm_nm, fl.spectrum_csv.as_deref(), fl_terms),
            make_layer(ph.peak_nm, ph.fwhm_nm, ph.spectrum_csv.as_deref(), ph_terms),
            true,
        )
    } else {
        let layer = make_layer(
            data.peak_nm,
            data.fwhm_nm,
            data.spectrum_csv.as_deref(),
            &data.decay_terms,
        );
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

/// Parse phosphor definitions from a TOML string with optional base path for CSV resolution.
pub fn load_phosphors_with_base_path(
    toml_str: &str,
    base_path: Option<&Path>,
) -> Result<Vec<PhosphorType>, toml::de::Error> {
    let table: BTreeMap<String, PhosphorData> = toml::from_str(toml_str)?;
    Ok(table
        .iter()
        .map(|(name, data)| build_phosphor(name, data, base_path))
        .collect())
}

/// Parse phosphor definitions from a TOML string (no CSV resolution).
pub fn load_phosphors(toml_str: &str) -> Result<Vec<PhosphorType>, toml::de::Error> {
    load_phosphors_with_base_path(toml_str, None)
}

/// Load phosphor definitions from a TOML file on disk.
/// CSV paths are resolved relative to the TOML file's parent directory.
pub fn load_phosphors_from_file(
    path: &Path,
) -> Result<Vec<PhosphorType>, Box<dyn std::error::Error>> {
    let contents = std::fs::read_to_string(path)?;
    let base = path.parent().unwrap_or(Path::new("."));
    Ok(load_phosphors_with_base_path(&contents, Some(base))?)
}
```

**Step 4: Run tests to verify they pass**

Run: `cargo test -p phosphor-data`
Expected: All tests PASS (existing + new).

**Step 5: Commit**

```bash
git add crates/phosphor-data/src/lib.rs
git commit -m "feat: wire spectrum_csv into phosphor building with base path

build_phosphor now accepts base_path for CSV resolution. When
spectrum_csv is set, reads and integrates the CSV file. Falls back to
gaussian_emission_weights when absent. load_phosphors_from_file
resolves CSVs relative to the TOML file's parent directory."
```

---

### Task 4: Update proc macro to pass base path

**Files:**

- Modify: `crates/phosphor-data-macro/src/lib.rs`

**Step 1: Update the proc macro to use `load_phosphors_with_base_path`**

In `crates/phosphor-data-macro/src/lib.rs`, change the `phosphor_table` function:

```rust
#[proc_macro]
pub fn phosphor_table(input: TokenStream) -> TokenStream {
    let lit = parse_macro_input!(input as LitStr);
    let path = resolve_path(&lit.value());

    let contents = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("Failed to read {}: {e}", path.display()));

    let base_path = path.parent().unwrap_or_else(|| {
        panic!("Cannot determine parent directory of {}", path.display())
    });

    let phosphors = phosphor_data::load_phosphors_with_base_path(&contents, Some(base_path))
        .unwrap_or_else(|e| panic!("Failed to parse {}: {e}", path.display()));

    let entries: Vec<String> = phosphors.iter().map(format_phosphor).collect();
    let body = entries.join(",\n    ");
    let code = format!("[\n    {body}\n]");

    code.parse()
        .expect("failed to parse generated phosphor array")
}
```

**Step 2: Run the full build to verify it compiles**

Run: `cargo build`
Expected: Compiles successfully. No behavior change yet since no phosphor uses `spectrum_csv`.

**Step 3: Commit**

```bash
git add crates/phosphor-data-macro/src/lib.rs
git commit -m "feat: proc macro passes base path for CSV spectrum resolution

phosphor_table! now calls load_phosphors_with_base_path with the TOML
file's parent directory, enabling spectrum_csv fields in built-in
phosphor definitions."
```

---

### Task 5: Add P1 CSV spectrum and update phosphors.toml

**Files:**

- Create: `data/spectra/p1_willemite.csv`
- Modify: `data/phosphors.toml`

**Step 1: Copy the P1 spectrum CSV into the repo**

Create `data/spectra/p1_willemite.csv` with the content from `~/Downloads/p1_willemite_emission_spectrum.csv` (the full file with comments and data).

**Step 2: Update `data/phosphors.toml` to reference the CSV**

Change the `[P1]` entry:

```toml
[P1]
description = "Medium persistence green. Standard general-purpose oscilloscope phosphor."
category = "general_purpose"
peak_nm = 525.0
spectrum_csv = "spectra/p1_willemite.csv"
relative_luminance = 50.0
relative_writing_speed = 60.0
```

Remove `fwhm_nm = 38.0` from P1 (no longer needed with CSV).

**Step 3: Build and run tests**

Run: `cargo test`
Expected: All tests PASS, including the proc macro baking the CSV-derived weights.

Run: `cargo run --release` (quick smoke test — P1 should still render green)

**Step 4: Commit**

```bash
git add data/spectra/p1_willemite.csv data/phosphors.toml
git commit -m "data: add P1 willemite emission spectrum CSV

Replaces Gaussian approximation (peak=525nm, FWHM=38nm) with measured
spectrophotometry data from Shionoya et al. Phosphor Handbook. 55 data
points from 440-600nm, re-digitized from modern spectrophotometry."
```

---

### Task 6: Verify end-to-end and run full test suite

**Files:** None (verification only)

**Step 1: Run the full test suite**

Run: `cargo test --workspace`
Expected: All tests PASS across all workspace crates.

**Step 2: Run the application and verify P1 visually**

Run: `cargo run --release`
Expected: P1 phosphor still renders as green. The spectral shape may be subtly different from the Gaussian approximation but should look correct.

**Step 3: Verify a Gaussian-only phosphor still works**

Switch to P31 or P11 in the UI and verify they render normally.
