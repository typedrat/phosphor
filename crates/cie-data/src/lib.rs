//! Proc macro that embeds the official CIE 1931 2-degree standard observer
//! color matching functions from a CSV file at compile time.
//!
//! Source: [CIE 018:2019](https://cie.co.at/datatable/cie-1931-colour-matching-functions-2-degree-observer)
//! DOI: [10.25039/CIE.DS.xvudnb9b](https://doi.org/10.25039/CIE.DS.xvudnb9b)

use proc_macro::TokenStream;
use std::path::PathBuf;
use syn::{LitStr, parse_macro_input};

/// Reads a CIE observer CSV and expands to an array literal of
/// `(f32, f64, f64, f64)` tuples: `(wavelength_nm, x_bar, y_bar, z_bar)`.
///
/// The path is resolved relative to the calling crate's `CARGO_MANIFEST_DIR`.
///
/// ```ignore
/// const CIE_1931_DATA: [(f32, f64, f64, f64); 471] =
///     cie_data::cie_1931_table!("data/CIE_xyz_1931_2deg.csv");
/// ```
#[proc_macro]
pub fn cie_1931_table(input: TokenStream) -> TokenStream {
    let lit = parse_macro_input!(input as LitStr);
    let path = resolve_path(&lit.value());

    let mut rdr = csv::ReaderBuilder::new()
        .has_headers(false)
        .from_path(&path)
        .unwrap_or_else(|e| panic!("Failed to open {}: {e}", path.display()));

    let mut entries = Vec::new();
    for result in rdr.records() {
        let record =
            result.unwrap_or_else(|e| panic!("CSV parse error in {}: {e}", path.display()));
        assert_eq!(
            record.len(),
            4,
            "Expected 4 columns in {}, got {} at record {}",
            path.display(),
            record.len(),
            entries.len() + 1,
        );

        let wl: f64 = record[0]
            .trim()
            .parse()
            .unwrap_or_else(|e| panic!("Invalid wavelength '{}': {e}", &record[0]));
        let x: f64 = record[1]
            .trim()
            .parse()
            .unwrap_or_else(|e| panic!("Invalid x_bar '{}': {e}", &record[1]));
        let y: f64 = record[2]
            .trim()
            .parse()
            .unwrap_or_else(|e| panic!("Invalid y_bar '{}': {e}", &record[2]));
        let z: f64 = record[3]
            .trim()
            .parse()
            .unwrap_or_else(|e| panic!("Invalid z_bar '{}': {e}", &record[3]));

        entries.push(format!("({wl}_f32, {x}_f64, {y}_f64, {z}_f64)"));
    }

    assert!(!entries.is_empty(), "CSV file {} is empty", path.display());

    let body = entries.join(",\n    ");
    let code = format!("[\n    {body}\n]");

    code.parse()
        .expect("failed to parse generated array literal")
}

/// Resolve a path relative to the calling crate's CARGO_MANIFEST_DIR.
fn resolve_path(relative: &str) -> PathBuf {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
    let path = PathBuf::from(manifest_dir).join(relative);
    assert!(
        path.exists(),
        "CIE data file not found at {}",
        path.display()
    );
    path
}
