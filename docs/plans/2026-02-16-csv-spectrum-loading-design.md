# CSV Spectrum Loading for Phosphors

## Problem

Phosphor emission spectra are currently approximated as Gaussians from `peak_nm` + `fwhm_nm`. Real phosphor spectra (from spectrophotometry) have asymmetries and fine structure that a single Gaussian cannot capture. We need the option to load measured spectral data from CSV files.

## Design

### CSV Format

Standard CSV with comment lines starting with `#`. Required columns: `wavelength_nm` and `rel_intensity`. Additional columns (e.g. `energy_eV`, `wavelength_angstrom`) are ignored. Intensity values are relative (arbitrary scale, normalized during integration).

Example:

```csv
# P1 Phosphor (Zn2SiO4:Mn2+) Emission Spectrum
wavelength_nm,wavelength_angstrom,energy_eV,rel_intensity
440,4400,2.818,0
...
525,5250,2.362,98
526,5260,2.357,100
...
600,6000,2.067,0
```

### TOML Integration

New optional `spectrum_csv` field on phosphor entries and layer sub-tables:

```toml
[P1]
description = "Medium persistence green."
category = "general_purpose"
peak_nm = 525.0
spectrum_csv = "spectra/p1_willemite.csv"
relative_luminance = 50.0
relative_writing_speed = 60.0
```

- When `spectrum_csv` is present, emission weights are computed from the CSV data instead of a Gaussian.
- `fwhm_nm` becomes unnecessary (and ignored) when `spectrum_csv` is set.
- `peak_nm` is kept for metadata display purposes.
- For dual-layer phosphors, `spectrum_csv` can appear in `[fluorescence]` and/or `[phosphorescence]` sub-tables.
- Path is resolved relative to the TOML file's parent directory.

### Integration Algorithm: `csv_to_emission_weights`

1. Parse CSV, skip `#` comment lines, find `wavelength_nm` and `rel_intensity` column indices from header.
2. Collect `(wavelength, intensity)` pairs, sorted by wavelength.
3. For each of the 16 spectral bands (25nm wide, 380-780nm), compute trapezoidal integral of the intensity curve over that band's range.
4. Normalize all 16 weights to sum to 1.0.
5. Return `[f32; SPECTRAL_BANDS]`.

Trapezoidal integration handles non-uniform wavelength spacing in the CSV naturally.

### Compile-Time vs Runtime

- **Built-in phosphors** (proc macro path): The proc macro reads the CSV at compile time. The TOML file's directory is used as the base path for resolving `spectrum_csv` paths.
- **User-added phosphors** (runtime loading): `load_phosphors_from_file` reads CSVs at runtime, resolving paths relative to the TOML file's location on disk.

### Changes by File

1. **`crates/phosphor-data/src/spectral.rs`**: Add `csv_to_emission_weights(csv_text: &str) -> Result<[f32; SPECTRAL_BANDS], CsvSpectrumError>`.

2. **`crates/phosphor-data/src/lib.rs`**:
   - Add `spectrum_csv: Option<String>` to `PhosphorData` and `LayerData`.
   - Add `base_path: Option<&Path>` parameter to `build_phosphor` (or a new variant) so CSV paths can be resolved.
   - When `spectrum_csv` is set, read the file and call `csv_to_emission_weights` instead of `gaussian_emission_weights`.
   - Update `load_phosphors` / `load_phosphors_from_file` to pass base path through.

3. **`crates/phosphor-data-macro/src/lib.rs`**: Pass the TOML file's parent directory as the base path when calling `load_phosphors`.

4. **`data/spectra/p1_willemite.csv`**: Add the P1 spectrum CSV to the repo.

5. **`data/phosphors.toml`**: Add `spectrum_csv` to P1, remove its `fwhm_nm`.
