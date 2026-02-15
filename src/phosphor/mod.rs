pub mod spectral;

pub use phosphor_data::{PhosphorCategory, PhosphorLayer, PhosphorType};

/// Built-in phosphor database, baked at compile time from data/phosphors.toml.
pub fn phosphor_database() -> Vec<PhosphorType> {
    phosphor_data_macro::phosphor_table!("data/phosphors.toml").to_vec()
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
        assert!(db.iter().any(|p| p.designation == "P7"));
        assert!(db.iter().any(|p| p.designation == "P11"));
        assert!(db.iter().any(|p| p.designation == "P31"));
    }

    #[test]
    fn dual_layer_phosphors_have_different_emission() {
        let db = phosphor_database();
        let p7 = db.iter().find(|p| p.designation == "P7").unwrap();
        assert!(p7.is_dual_layer);
        assert_ne!(
            p7.fluorescence.emission_weights,
            p7.phosphorescence.emission_weights
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
