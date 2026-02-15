/// Number of spectral bands across the visible spectrum.
/// Changing this value propagates through the entire pipeline.
pub const SPECTRAL_BANDS: usize = 16;

/// Visible spectrum range in nanometers.
pub const WAVELENGTH_MIN: f32 = 380.0;
pub const WAVELENGTH_MAX: f32 = 780.0;
pub const BAND_WIDTH: f32 = (WAVELENGTH_MAX - WAVELENGTH_MIN) / SPECTRAL_BANDS as f32;

/// Returns the (min, max) wavelength range in nm for a given band index.
pub fn band_range(band: usize) -> (f32, f32) {
    let min = WAVELENGTH_MIN + band as f32 * BAND_WIDTH;
    let max = min + BAND_WIDTH;
    (min, max)
}

/// Returns the center wavelength in nm for a given band index.
pub fn band_center(band: usize) -> f32 {
    let (min, max) = band_range(band);
    (min + max) / 2.0
}

/// CIE 1931 2-degree standard observer color matching functions.
/// 1nm steps, 360–830nm. Each entry: (wavelength_nm, x_bar, y_bar, z_bar).
///
/// Source: CIE 018:2019 (DOI: 10.25039/CIE.DS.xvudnb9b)
const CIE_1931_DATA: [(f32, f64, f64, f64); 471] =
    cie_data::cie_1931_table!("data/CIE_xyz_1931_2deg.csv");

/// CIE 1931 color matching function values (x_bar, y_bar, z_bar),
/// pre-integrated over each spectral band. Used to convert spectral
/// energy to XYZ tristimulus values.
///
/// Returns a [SPECTRAL_BANDS] array of (x, y, z) tuples.
pub fn cie_integration_weights() -> [(f32, f32, f32); SPECTRAL_BANDS] {
    let mut weights = [(0.0f32, 0.0f32, 0.0f32); SPECTRAL_BANDS];

    for band in 0..SPECTRAL_BANDS {
        let (band_min, band_max) = band_range(band);
        let mut sum_x = 0.0f64;
        let mut sum_y = 0.0f64;
        let mut sum_z = 0.0f64;
        let mut count = 0u32;

        for &(wl, x, y, z) in &CIE_1931_DATA {
            if wl >= band_min && wl < band_max {
                sum_x += x;
                sum_y += y;
                sum_z += z;
                count += 1;
            }
        }

        if count > 0 {
            // Average the CIE values within this band, then multiply by band width
            // to get the integrated weight.
            let n = count as f64;
            weights[band] = (
                (sum_x / n * BAND_WIDTH as f64) as f32,
                (sum_y / n * BAND_WIDTH as f64) as f32,
                (sum_z / n * BAND_WIDTH as f64) as f32,
            );
        }
    }

    weights
}

/// Compute emission weights for a Gaussian emission peak.
/// Returns normalized weights (sum = 1.0) across SPECTRAL_BANDS.
///
/// `peak_nm`: center wavelength of emission in nm
/// `fwhm_nm`: full width at half maximum in nm
pub fn gaussian_emission_weights(peak_nm: f32, fwhm_nm: f32) -> [f32; SPECTRAL_BANDS] {
    let sigma = fwhm_nm / 2.355; // FWHM = 2*sqrt(2*ln2)*sigma ≈ 2.355*sigma
    let mut weights = [0.0f32; SPECTRAL_BANDS];

    for i in 0..SPECTRAL_BANDS {
        let center = band_center(i);
        let d = (center - peak_nm) / sigma;
        weights[i] = (-0.5 * d * d).exp();
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
