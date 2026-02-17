pub mod spectral;

pub use phosphor_data::PhosphorType;

/// Built-in phosphor database, baked at compile time from data/phosphors.toml.
/// Returned sorted by designation in natural order (P1 < P2 < P10).
pub fn phosphor_database() -> Vec<PhosphorType> {
    let mut db = phosphor_data_macro::phosphor_table!("data/phosphors.toml").to_vec();
    db.sort_by(|a, b| natord::compare(&a.designation, &b.designation));
    db
}

/// Load additional phosphors from a TOML file on disk.
pub fn load_phosphors(
    path: &std::path::Path,
) -> anyhow::Result<Vec<PhosphorType>, Box<dyn std::error::Error>> {
    phosphor_data::load_phosphors_from_file(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn database_has_common_phosphors() {
        let db = phosphor_database();
        assert!(db.iter().any(|p| p.designation == "P1"));
        assert!(db.iter().any(|p| p.designation == "P11"));
        assert!(db.iter().any(|p| p.designation == "P31"));
    }

    #[test]
    fn dual_layer_parsing_from_toml() {
        let toml_str = r#"
[Test]
description = "Dual-layer test phosphor."
category = "long_decay_sulfide"
dual_layer = true
peak_nm = 450.0
relative_luminance = 35.0
relative_writing_speed = 65.0

[[Test.decay_terms]]
type = "exponential"
amplitude = 1.0
tau = 0.001

[Test.fluorescence]
peak_nm = 450.0
fwhm_nm = 30.0

[Test.phosphorescence]
peak_nm = 530.0
fwhm_nm = 50.0
"#;
        let phosphors = phosphor_data::load_phosphors(toml_str).unwrap();
        let p = &phosphors[0];
        assert!(p.is_dual_layer);
        assert_ne!(
            p.fluorescence.emission_weights,
            p.phosphorescence.emission_weights
        );
    }

    #[test]
    fn single_layer_phosphors_share_emission() {
        let db = phosphor_database();
        let p31 = db.iter().find(|p| p.designation == "P31").unwrap();
        assert!(!p31.is_dual_layer);
        assert_eq!(
            p31.fluorescence.emission_weights,
            p31.phosphorescence.emission_weights
        );
    }

    #[test]
    fn emission_weights_are_normalized() {
        let db = phosphor_database();
        for p in &db {
            let sum_f: f32 = p.fluorescence.emission_weights.iter().sum();
            let sum_p: f32 = p.phosphorescence.emission_weights.iter().sum();
            assert!(
                (sum_f - 1.0).abs() < 0.01,
                "{} fluorescence: {sum_f}",
                p.designation
            );
            assert!(
                (sum_p - 1.0).abs() < 0.01,
                "{} phosphorescence: {sum_p}",
                p.designation
            );
        }
    }
}
