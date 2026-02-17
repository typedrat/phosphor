pub const SPECTRAL_BANDS: usize = 16;
pub const WAVELENGTH_MIN: f32 = 380.0;
pub const WAVELENGTH_MAX: f32 = 780.0;
pub const BAND_WIDTH: f32 = (WAVELENGTH_MAX - WAVELENGTH_MIN) / SPECTRAL_BANDS as f32;

pub const fn band_range(band: usize) -> (f32, f32) {
    let min = WAVELENGTH_MIN + band as f32 * BAND_WIDTH;
    let max = min + BAND_WIDTH;
    (min, max)
}

pub const fn band_center(band: usize) -> f32 {
    WAVELENGTH_MIN + (band as f32 + 0.5) * BAND_WIDTH
}

// --- CSV spectrum parsing ---

#[derive(Debug)]
pub enum CsvSpectrumError {
    MissingColumn(&'static str),
    ParseFloat {
        line: usize,
        col: &'static str,
        err: std::num::ParseFloatError,
    },
    TooFewPoints,
    ZeroIntensity,
}

impl std::fmt::Display for CsvSpectrumError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CsvSpectrumError::MissingColumn(col) => {
                write!(f, "missing required column: {col}")
            }
            CsvSpectrumError::ParseFloat { line, col, err } => {
                write!(f, "line {line}, column '{col}': {err}")
            }
            CsvSpectrumError::TooFewPoints => {
                write!(f, "need at least 2 data points for interpolation")
            }
            CsvSpectrumError::ZeroIntensity => {
                write!(f, "total integrated intensity is zero")
            }
        }
    }
}

impl std::error::Error for CsvSpectrumError {}

/// Parse a CSV spectrum (with `wavelength_nm` and `rel_intensity` columns) into
/// normalized emission weights for [`SPECTRAL_BANDS`] spectral bands.
///
/// The CSV may contain comment lines starting with `#`, extra columns (ignored),
/// and non-uniform wavelength spacing. Integration uses the trapezoidal rule with
/// line segments clipped to band boundaries.
pub fn csv_to_emission_weights(csv_text: &str) -> Result<[f32; SPECTRAL_BANDS], CsvSpectrumError> {
    // 1. Iterate all lines, tracking 1-indexed line numbers, skip comments inline
    let mut line_iter = csv_text.lines().enumerate();

    // 2. Parse header row, find column indices (skip comments/empty lines)
    let header = loop {
        match line_iter.next() {
            Some((_idx, line)) => {
                let trimmed = line.trim();
                if !trimmed.is_empty() && !trimmed.starts_with('#') {
                    break line;
                }
            }
            None => return Err(CsvSpectrumError::TooFewPoints),
        }
    };
    let columns: Vec<&str> = header.split(',').map(|s| s.trim()).collect();

    let wl_idx = columns
        .iter()
        .position(|&c| c == "wavelength_nm")
        .ok_or(CsvSpectrumError::MissingColumn("wavelength_nm"))?;
    let int_idx = columns
        .iter()
        .position(|&c| c == "rel_intensity")
        .ok_or(CsvSpectrumError::MissingColumn("rel_intensity"))?;

    // 3. Parse data rows into (wavelength, intensity) pairs
    let mut points: Vec<(f32, f32)> = Vec::new();
    for (line_idx, line) in line_iter {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let line_num = line_idx + 1; // 1-indexed
        let fields: Vec<&str> = line.split(',').map(|s| s.trim()).collect();
        let wl: f32 = fields.get(wl_idx).unwrap_or(&"").parse().map_err(|err| {
            CsvSpectrumError::ParseFloat {
                line: line_num,
                col: "wavelength_nm",
                err,
            }
        })?;
        let intensity: f32 = fields
            .get(int_idx)
            .unwrap_or(&"")
            .parse::<f32>()
            .map_err(|err| CsvSpectrumError::ParseFloat {
                line: line_num,
                col: "rel_intensity",
                err,
            })?
            .max(0.0);
        points.push((wl, intensity));
    }

    // 4. Validate minimum points
    if points.len() < 2 {
        return Err(CsvSpectrumError::TooFewPoints);
    }

    // 5. Sort by wavelength
    points.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

    // 6. For each spectral band, compute trapezoidal integral with clipping
    let mut weights = [0.0f32; SPECTRAL_BANDS];

    for (band, weight) in weights.iter_mut().enumerate() {
        let (band_lo, band_hi) = band_range(band);
        let mut integral = 0.0f32;

        for i in 0..points.len() - 1 {
            let (x0, y0) = points[i];
            let (x1, y1) = points[i + 1];

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

            // Trapezoidal rule: area = (y0 + y1) / 2 * dx
            let dx = cx1 - cx0;
            if dx > 0.0 {
                integral += (cy0 + cy1) * 0.5 * dx;
            }
        }

        *weight = integral;
    }

    // 7. Normalize weights to sum to 1.0
    let total: f32 = weights.iter().sum();
    if total <= 0.0 {
        return Err(CsvSpectrumError::ZeroIntensity);
    }
    for w in &mut weights {
        *w /= total;
    }

    Ok(weights)
}

pub fn gaussian_emission_weights(peak_nm: f32, fwhm_nm: f32) -> [f32; SPECTRAL_BANDS] {
    let sigma = fwhm_nm / 2.355;
    let mut weights = [0.0f32; SPECTRAL_BANDS];

    for (i, w) in weights.iter_mut().enumerate().take(SPECTRAL_BANDS) {
        let center = band_center(i);
        let d = (center - peak_nm) / sigma;
        *w = (-0.5 * d * d).exp();
    }

    let sum: f32 = weights.iter().sum();
    if sum > 0.0 {
        for w in &mut weights {
            *w /= sum;
        }
    }

    weights
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_to_emission_weights_basic() {
        // Narrow spike at 525nm — should land in band 5 (505–530nm).
        // Two points tightly around 525nm with high intensity.
        let csv = "\
wavelength_nm,rel_intensity
524.0,1.0
525.0,1.0
526.0,1.0
";
        let weights = csv_to_emission_weights(csv).unwrap();

        // Band 5 (505-530nm) should have essentially all the weight
        let sum: f32 = weights.iter().sum();
        assert!(
            (sum - 1.0).abs() < 1e-5,
            "weights should sum to 1.0, got {sum}"
        );

        // Band 5 should dominate
        assert!(
            weights[5] > 0.99,
            "band 5 should have >99% of weight, got {}",
            weights[5]
        );
    }

    #[test]
    fn csv_to_emission_weights_uniform() {
        // Flat spectrum across 380-780nm — all bands should get roughly equal weight.
        let mut csv = String::from("wavelength_nm,rel_intensity\n");
        for wl in (380..=780).step_by(5) {
            csv.push_str(&format!("{wl},1.0\n"));
        }
        let weights = csv_to_emission_weights(&csv).unwrap();

        let expected = 1.0 / SPECTRAL_BANDS as f32;
        for (i, &w) in weights.iter().enumerate() {
            assert!(
                (w - expected).abs() < 0.02,
                "band {i}: expected ~{expected:.4}, got {w:.4}"
            );
        }
    }

    #[test]
    fn csv_to_emission_weights_extra_columns_ignored() {
        // CSV with extra columns — only wavelength_nm and rel_intensity should be used.
        let csv = "\
wavelength_nm,wavelength_angstrom,energy_eV,rel_intensity
500.0,5000.0,2.48,0.5
510.0,5100.0,2.43,1.0
520.0,5200.0,2.38,0.5
";
        let weights = csv_to_emission_weights(csv).unwrap();
        let sum: f32 = weights.iter().sum();
        assert!(
            (sum - 1.0).abs() < 1e-5,
            "weights should sum to 1.0, got {sum}"
        );
        // All energy is in the 500-530nm range (bands 4-5)
        let band_4_5 = weights[4] + weights[5];
        assert!(
            band_4_5 > 0.99,
            "bands 4+5 should capture >99% of energy, got {band_4_5}"
        );
    }

    #[test]
    fn csv_to_emission_weights_missing_column_errors() {
        // CSV without rel_intensity column should error
        let csv = "\
wavelength_nm,something_else
500.0,1.0
510.0,2.0
";
        let result = csv_to_emission_weights(csv);
        assert!(result.is_err());
        match result.unwrap_err() {
            CsvSpectrumError::MissingColumn(col) => {
                assert_eq!(col, "rel_intensity");
            }
            other => panic!("expected MissingColumn, got {other:?}"),
        }
    }

    #[test]
    fn csv_to_emission_weights_too_few_points() {
        let csv = "wavelength_nm,rel_intensity\n500.0,1.0\n";
        assert!(matches!(
            csv_to_emission_weights(csv),
            Err(CsvSpectrumError::TooFewPoints)
        ));
    }

    #[test]
    fn csv_to_emission_weights_zero_intensity() {
        let csv = "wavelength_nm,rel_intensity\n500.0,0.0\n510.0,0.0\n";
        assert!(matches!(
            csv_to_emission_weights(csv),
            Err(CsvSpectrumError::ZeroIntensity)
        ));
    }
}
