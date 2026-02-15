pub use phosphor_data::spectral::{
    BAND_WIDTH, SPECTRAL_BANDS, WAVELENGTH_MAX, WAVELENGTH_MIN, band_center, band_range,
    gaussian_emission_weights,
};

/// CIE 1931 2-degree standard observer color matching functions.
/// 1nm steps, 360-830nm. Each entry: (wavelength_nm, x_bar, y_bar, z_bar).
///
/// Source: CIE 018:2019 (DOI: 10.25039/CIE.DS.xvudnb9b)
const CIE_1931_DATA: [(f32, f64, f64, f64); 471] =
    cie_data::cie_1931_table!("data/CIE_xyz_1931_2deg.csv");

/// CIE 1931 color matching function values (x_bar, y_bar, z_bar),
/// pre-integrated over each spectral band. Used to convert spectral
/// energy to XYZ tristimulus values.
///
/// Computed at compile time as a [SPECTRAL_BANDS] array of (x, y, z) tuples.
pub const CIE_INTEGRATION_WEIGHTS: [(f32, f32, f32); SPECTRAL_BANDS] = {
    let mut weights = [(0.0f32, 0.0f32, 0.0f32); SPECTRAL_BANDS];

    let mut band = 0;
    while band < SPECTRAL_BANDS {
        let (band_min, band_max) = band_range(band);
        let mut sum_x = 0.0f64;
        let mut sum_y = 0.0f64;
        let mut sum_z = 0.0f64;
        let mut count = 0u32;

        let mut i = 0;
        while i < CIE_1931_DATA.len() {
            let (wl, x, y, z) = CIE_1931_DATA[i];
            if wl >= band_min && wl < band_max {
                sum_x += x;
                sum_y += y;
                sum_z += z;
                count += 1;
            }
            i += 1;
        }

        if count > 0 {
            let n = count as f64;
            weights[band] = (
                (sum_x / n * BAND_WIDTH as f64) as f32,
                (sum_y / n * BAND_WIDTH as f64) as f32,
                (sum_z / n * BAND_WIDTH as f64) as f32,
            );
        }

        band += 1;
    }

    weights
};

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
        for (x, y, z) in &CIE_INTEGRATION_WEIGHTS {
            assert!(x.is_finite());
            assert!(y.is_finite());
            assert!(z.is_finite());
            assert!(*y >= 0.0);
        }
    }
}
