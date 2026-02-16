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
    decay_terms: Vec<DecayTerm>,
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
    let make_layer = |peak: f32, fwhm: f32, terms: &[DecayTerm]| -> PhosphorLayer {
        PhosphorLayer {
            emission_weights: spectral::gaussian_emission_weights(peak, fwhm),
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
        let fl_terms = if fl.decay_terms.is_empty() {
            &data.decay_terms
        } else {
            &fl.decay_terms
        };
        let ph_terms = if ph.decay_terms.is_empty() {
            &data.decay_terms
        } else {
            &ph.decay_terms
        };
        (
            make_layer(fl.peak_nm, fl.fwhm_nm, fl_terms),
            make_layer(ph.peak_nm, ph.fwhm_nm, ph_terms),
            true,
        )
    } else {
        let fwhm = data
            .fwhm_nm
            .unwrap_or_else(|| panic!("{designation}: single-layer phosphor missing fwhm_nm"));
        let layer = make_layer(data.peak_nm, fwhm, &data.decay_terms);
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

/// Result of classifying a phosphor's decay terms into tiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecayClassification {
    /// Exponential terms with tau < tau_cutoff (tier 1: computed analytically).
    pub instant_exp_count: usize,
    /// Exponential terms with tau >= tau_cutoff (tier 2: multiplicative per-frame).
    pub slow_exp_count: usize,
    /// Whether any power-law term exists (tier 3: elapsed-time tracking).
    pub has_power_law: bool,
}

impl DecayClassification {
    /// Total accumulation buffer layers needed for this phosphor layer.
    pub fn accum_layers(&self) -> usize {
        let mut layers = self.slow_exp_count * SPECTRAL_BANDS;
        if self.has_power_law {
            layers += SPECTRAL_BANDS + 1;
        }
        layers
    }
}

/// Classify decay terms into tiers based on a time constant cutoff.
pub fn classify_decay_terms(terms: &[DecayTerm], tau_cutoff: f32) -> DecayClassification {
    let mut instant = 0;
    let mut slow = 0;
    let mut power_law = false;

    for term in terms {
        match term {
            DecayTerm::Exponential { tau, .. } => {
                if *tau < tau_cutoff {
                    instant += 1;
                } else {
                    slow += 1;
                }
            }
            DecayTerm::PowerLaw { .. } => {
                power_law = true;
            }
        }
    }

    DecayClassification {
        instant_exp_count: instant,
        slow_exp_count: slow,
        has_power_law: power_law,
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

    #[test]
    fn parse_explicit_decay_terms_from_toml() {
        let toml_str = r#"
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
"#;
        let phosphors = load_phosphors(toml_str).unwrap();
        assert_eq!(phosphors.len(), 1);
        let p1 = &phosphors[0];
        assert_eq!(p1.fluorescence.decay_terms.len(), 2);
        match p1.fluorescence.decay_terms[0] {
            DecayTerm::Exponential { amplitude, tau } => {
                assert!((amplitude - 6.72).abs() < 1e-6);
                assert!((tau - 0.00288).abs() < 1e-8);
            }
            _ => panic!("expected exponential"),
        }
    }

    #[test]
    fn parse_power_law_term_from_toml() {
        let toml_str = r#"
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
"#;
        let phosphors = load_phosphors(toml_str).unwrap();
        let p31 = &phosphors[0];
        assert_eq!(p31.fluorescence.decay_terms.len(), 2);
        match p31.fluorescence.decay_terms[0] {
            DecayTerm::PowerLaw {
                amplitude,
                alpha,
                beta,
            } => {
                assert!((amplitude - 2.1e-4).abs() < 1e-10);
                assert!((alpha - 5.5e-6).abs() < 1e-12);
                assert!((beta - 1.1).abs() < 1e-6);
            }
            _ => panic!("expected power_law"),
        }
    }

    #[test]
    fn classify_p1_all_slow_exponential() {
        let terms = vec![
            DecayTerm::Exponential {
                amplitude: 6.72,
                tau: 0.00288,
            },
            DecayTerm::Exponential {
                amplitude: 1.0,
                tau: 0.0151,
            },
        ];
        let class = classify_decay_terms(&terms, 1e-4);
        assert_eq!(class.instant_exp_count, 0);
        assert_eq!(class.slow_exp_count, 2);
        assert!(!class.has_power_law);
    }

    #[test]
    fn classify_p31_power_law_plus_instant() {
        let terms = vec![
            DecayTerm::PowerLaw {
                amplitude: 2.1e-4,
                alpha: 5.5e-6,
                beta: 1.1,
            },
            DecayTerm::Exponential {
                amplitude: 90.0,
                tau: 31.8e-9,
            },
            DecayTerm::Exponential {
                amplitude: 100.0,
                tau: 227e-9,
            },
            DecayTerm::Exponential {
                amplitude: 37.0,
                tau: 1.06e-6,
            },
        ];
        let class = classify_decay_terms(&terms, 1e-4);
        assert_eq!(class.instant_exp_count, 3);
        assert_eq!(class.slow_exp_count, 0);
        assert!(class.has_power_law);
    }
}
