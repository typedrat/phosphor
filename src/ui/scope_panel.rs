use crate::phosphor::PhosphorType;

pub fn scope_panel(
    ui: &mut egui::Ui,
    phosphors: &[PhosphorType],
    phosphor_index: &mut usize,
    intensity: &mut f32,
    focus: &mut f32,
) {
    ui.heading("Phosphor");

    let current = &phosphors[*phosphor_index];
    egui::ComboBox::from_id_salt("phosphor_type")
        .selected_text(&current.designation)
        .show_ui(ui, |ui| {
            for (i, p) in phosphors.iter().enumerate() {
                ui.selectable_value(
                    phosphor_index,
                    i,
                    format!("{} — {}", p.designation, p.description),
                );
            }
        });

    ui.separator();

    ui.label("Intensity");
    ui.add(egui::Slider::new(intensity, 0.1..=10.0).logarithmic(true));

    ui.label("Focus");
    ui.add(egui::Slider::new(focus, 0.5..=5.0).text("px"));
}
