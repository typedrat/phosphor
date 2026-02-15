use crate::phosphor::PhosphorType;

pub fn scope_panel(
    ctx: &egui::Context,
    phosphors: &[PhosphorType],
    phosphor_index: &mut usize,
    intensity: &mut f32,
    focus: &mut f32,
    faceplate_scatter_intensity: &mut f32,
) {
    egui::SidePanel::left("scope_panel")
        .default_width(220.0)
        .show(ctx, |ui| {
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

            ui.label("Faceplate Scatter");
            ui.add(egui::Slider::new(faceplate_scatter_intensity, 0.0..=1.0));

            ui.separator();

            ui.label(format!("FPS: {:.0}", 1.0 / ctx.input(|i| i.predicted_dt)));
        });
}
