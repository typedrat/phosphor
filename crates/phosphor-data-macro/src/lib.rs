use std::path::PathBuf;

use proc_macro::TokenStream;
use syn::{LitStr, parse_macro_input};

use phosphor_data::spectral::SPECTRAL_BANDS;

fn format_weights(w: &[f32; SPECTRAL_BANDS]) -> String {
    let inner: Vec<String> = w.iter().map(|v| format!("{v}_f32")).collect();
    format!("[{}]", inner.join(", "))
}

fn format_category(cat: &phosphor_data::PhosphorCategory) -> &'static str {
    match cat {
        phosphor_data::PhosphorCategory::GeneralPurpose => {
            "phosphor_data::PhosphorCategory::GeneralPurpose"
        }
        phosphor_data::PhosphorCategory::ShortDecay => {
            "phosphor_data::PhosphorCategory::ShortDecay"
        }
        phosphor_data::PhosphorCategory::VideoDisplay => {
            "phosphor_data::PhosphorCategory::VideoDisplay"
        }
        phosphor_data::PhosphorCategory::LongDecaySulfide => {
            "phosphor_data::PhosphorCategory::LongDecaySulfide"
        }
    }
}

fn format_decay_term(term: &phosphor_data::DecayTerm) -> String {
    match term {
        phosphor_data::DecayTerm::Exponential { amplitude, tau } => {
            format!(
                "phosphor_data::DecayTerm::Exponential {{ amplitude: {amplitude}_f32, tau: {tau}_f32 }}"
            )
        }
        phosphor_data::DecayTerm::PowerLaw {
            amplitude,
            alpha,
            beta,
        } => {
            format!(
                "phosphor_data::DecayTerm::PowerLaw {{ amplitude: {amplitude}_f32, alpha: {alpha}_f32, beta: {beta}_f32 }}"
            )
        }
    }
}

fn format_layer(layer: &phosphor_data::PhosphorLayer) -> String {
    let terms: Vec<String> = layer.decay_terms.iter().map(format_decay_term).collect();
    format!(
        "phosphor_data::PhosphorLayer {{ emission_weights: {}, decay_terms: vec![{}] }}",
        format_weights(&layer.emission_weights),
        terms.join(", "),
    )
}

fn format_phosphor(p: &phosphor_data::PhosphorType) -> String {
    format!(
        r#"phosphor_data::PhosphorType {{
        designation: String::from("{designation}"),
        description: String::from("{description}"),
        category: {category},
        is_dual_layer: {dual},
        fluorescence: {fl},
        phosphorescence: {ph},
        peak_wavelength_nm: {peak}_f32,
        relative_luminance: {lum}_f32,
        relative_writing_speed: {ws}_f32,
    }}"#,
        designation = p.designation,
        description = p.description,
        category = format_category(&p.category),
        dual = p.is_dual_layer,
        fl = format_layer(&p.fluorescence),
        ph = format_layer(&p.phosphorescence),
        peak = p.peak_wavelength_nm,
        lum = p.relative_luminance,
        ws = p.relative_writing_speed,
    )
}

/// Reads a phosphor database TOML file and expands to an array literal
/// of `phosphor_data::PhosphorType` structs with pre-computed emission
/// weights and fitted decay constants.
///
/// The path is resolved relative to the calling crate's `CARGO_MANIFEST_DIR`.
///
/// The calling crate must depend on `phosphor-data` for the types.
///
/// ```ignore
/// let db: &[phosphor_data::PhosphorType] =
///     &phosphor_data_macro::phosphor_table!("data/phosphors.toml");
/// ```
#[proc_macro]
pub fn phosphor_table(input: TokenStream) -> TokenStream {
    let lit = parse_macro_input!(input as LitStr);
    let path = resolve_path(&lit.value());

    let contents = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("Failed to read {}: {e}", path.display()));

    let base_path = path
        .parent()
        .unwrap_or_else(|| panic!("Cannot determine parent directory of {}", path.display()));

    let phosphors = phosphor_data::load_phosphors_with_base_path(&contents, Some(base_path))
        .unwrap_or_else(|e| panic!("Failed to parse {}: {e}", path.display()));

    let entries: Vec<String> = phosphors.iter().map(format_phosphor).collect();
    let body = entries.join(",\n    ");
    let code = format!("[\n    {body}\n]");

    code.parse()
        .expect("failed to parse generated phosphor array")
}

fn resolve_path(relative: &str) -> PathBuf {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
    let path = PathBuf::from(manifest_dir).join(relative);
    assert!(
        path.exists(),
        "Phosphor data file not found at {}",
        path.display()
    );
    path
}
