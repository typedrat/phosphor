pub fn engineer_panel(
    ui: &mut egui::Ui,
    glass_tint: &mut [f32; 3],
    curvature: &mut f32,
    edge_falloff: &mut f32,
) {
    ui.heading("Glass Faceplate");

    ui.label("Tint");
    ui.color_edit_button_rgb(glass_tint);

    ui.label("Curvature");
    ui.add(egui::Slider::new(curvature, 0.0..=0.5));

    ui.label("Edge Falloff");
    ui.add(egui::Slider::new(edge_falloff, 0.0..=1.0));
}
