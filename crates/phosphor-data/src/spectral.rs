pub const SPECTRAL_BANDS: usize = 16;
pub const WAVELENGTH_MIN: f32 = 380.0;
pub const WAVELENGTH_MAX: f32 = 780.0;
pub const BAND_WIDTH: f32 = (WAVELENGTH_MAX - WAVELENGTH_MIN) / SPECTRAL_BANDS as f32;

pub fn band_range(band: usize) -> (f32, f32) {
    let min = WAVELENGTH_MIN + band as f32 * BAND_WIDTH;
    let max = min + BAND_WIDTH;
    (min, max)
}

pub fn band_center(band: usize) -> f32 {
    WAVELENGTH_MIN + (band as f32 + 0.5) * BAND_WIDTH
}

pub fn gaussian_emission_weights(peak_nm: f32, fwhm_nm: f32) -> [f32; SPECTRAL_BANDS] {
    let sigma = fwhm_nm / 2.355;
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
