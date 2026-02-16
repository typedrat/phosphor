pub mod decay;
pub mod spectral;

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;
use spectral::SPECTRAL_BANDS;

// --- Public types ---

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(tag = "type")]
pub enum DecayTerm {
    #[serde(rename = "exponential")]
    Exponential { amplitude: f32, tau: f32 },
    #[serde(rename = "power_law")]
    PowerLaw {
        amplitude: f32,
        alpha: f32,
        beta: f32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhosphorCategory {
    GeneralPurpose,
    ShortDecay,
    VideoDisplay,
    LongDecaySulfide,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PhosphorLayer {
    pub emission_weights: [f32; SPECTRAL_BANDS],
    pub decay_terms: Vec<DecayTerm>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PhosphorType {
    pub designation: String,
    pub description: String,
    pub category: PhosphorCategory,
    pub is_dual_layer: bool,
    pub fluorescence: PhosphorLayer,
    pub phosphorescence: PhosphorLayer,
    pub peak_wavelength_nm: f32,
    pub relative_luminance: f32,
    pub relative_writing_speed: f32,
}

// --- TOML deserialization ---

#[derive(Debug, Deserialize)]
struct LayerData {
    peak_nm: f32,
    fwhm_nm: f32,
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
    t_10pct: f32,
    t_1pct: f32,
    t_01pct: f32,
    relative_luminance: f32,
    relative_writing_speed: f32,
    fluorescence: Option<LayerData>,
    phosphorescence: Option<LayerData>,
}

fn parse_category(s: &str) -> PhosphorCategory {
    match s {
        "general_purpose" => PhosphorCategory::GeneralPurpose,
        "short_decay" => PhosphorCategory::ShortDecay,
        "video_display" => PhosphorCategory::VideoDisplay,
        "long_decay_sulfide" => PhosphorCategory::LongDecaySulfide,
        other => panic!("Unknown phosphor category: {other}"),
    }
}

fn build_phosphor(designation: &str, data: &PhosphorData) -> PhosphorType {
    let (tau_fast, tau_slow, a_fast, _a_slow) =
        decay::fit_decay(data.t_10pct, data.t_1pct, data.t_01pct);

    let make_layer = |peak: f32, fwhm: f32| -> PhosphorLayer {
        PhosphorLayer {
            emission_weights: spectral::gaussian_emission_weights(peak, fwhm),
            decay_terms: vec![
                DecayTerm::Exponential {
                    amplitude: a_fast,
                    tau: tau_fast,
                },
                DecayTerm::Exponential {
                    amplitude: 1.0 - a_fast,
                    tau: tau_slow,
                },
            ],
        }
    };

    let (fluorescence, phosphorescence, is_dual_layer) = if data.dual_layer {
        let fl = data.fluorescence.as_ref().unwrap_or_else(|| {
            panic!("{designation}: dual_layer = true but missing [fluorescence]")
        });
        let ph = data.phosphorescence.as_ref().unwrap_or_else(|| {
            panic!("{designation}: dual_layer = true but missing [phosphorescence]")
        });
        (
            make_layer(fl.peak_nm, fl.fwhm_nm),
            make_layer(ph.peak_nm, ph.fwhm_nm),
            true,
        )
    } else {
        let fwhm = data
            .fwhm_nm
            .unwrap_or_else(|| panic!("{designation}: single-layer phosphor missing fwhm_nm"));
        let layer = make_layer(data.peak_nm, fwhm);
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

/// Parse phosphor definitions from a TOML string.
pub fn load_phosphors(toml_str: &str) -> Result<Vec<PhosphorType>, toml::de::Error> {
    let table: BTreeMap<String, PhosphorData> = toml::from_str(toml_str)?;
    Ok(table
        .iter()
        .map(|(name, data)| build_phosphor(name, data))
        .collect())
}

/// Load phosphor definitions from a TOML file on disk.
pub fn load_phosphors_from_file(
    path: &Path,
) -> Result<Vec<PhosphorType>, Box<dyn std::error::Error>> {
    let contents = std::fs::read_to_string(path)?;
    Ok(load_phosphors(&contents)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decay_term_exponential_fields() {
        let term = DecayTerm::Exponential {
            amplitude: 6.72,
            tau: 0.00288,
        };
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
        let term = DecayTerm::PowerLaw {
            amplitude: 2.1e-4,
            alpha: 5.5e-6,
            beta: 1.1,
        };
        match term {
            DecayTerm::PowerLaw {
                amplitude,
                alpha,
                beta,
            } => {
                assert!((amplitude - 2.1e-4).abs() < 1e-10);
                assert!((alpha - 5.5e-6).abs() < 1e-12);
                assert!((beta - 1.1).abs() < 1e-6);
            }
            _ => panic!("expected PowerLaw"),
        }
    }
}
