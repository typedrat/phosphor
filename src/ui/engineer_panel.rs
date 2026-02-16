use phosphor_data::spectral::{SPECTRAL_BANDS, band_center};

use crate::gpu::composite::TonemapMode;
use crate::gpu::profiler::{HISTORY_CAP, NUM_SEGMENTS, SEGMENT_NAMES, TimingHistory};
use crate::phosphor::PhosphorType;

pub struct EngineerState {
    // Beam
    pub sigma_core: f32,
    pub sigma_halo: f32,
    pub halo_fraction: f32,
    pub space_charge: f32,
    pub accel_voltage: f32,
    // Decay
    pub tau_fast: f32,
    pub tau_slow: f32,
    pub a_fast: f32,
    // Faceplate scatter
    pub scatter_threshold: f32,
    pub scatter_sigma: f32,
    pub scatter_intensity: f32,
    // Composite / display
    pub tonemap_mode: TonemapMode,
    pub exposure: f32,
    pub white_point: f32,
    pub glass_tint: [f32; 3],
    pub curvature: f32,
    pub edge_falloff: f32,
    // Resolution
    pub accum_resolution_scale: f32,
}

impl Default for EngineerState {
    fn default() -> Self {
        Self {
            sigma_core: 1.5,
            sigma_halo: 6.0,
            halo_fraction: 0.03,
            space_charge: 0.0,
            accel_voltage: 10.0,
            tau_fast: 0.012,
            tau_slow: 0.040,
            a_fast: 0.7,
            scatter_threshold: 0.5,
            scatter_sigma: 4.0,
            scatter_intensity: 0.15,
            tonemap_mode: TonemapMode::default(),
            exposure: 1.0,
            white_point: 1.0,
            glass_tint: [0.92, 0.95, 0.92],
            curvature: 0.0,
            edge_falloff: 0.0,
            accum_resolution_scale: 1.0,
        }
    }
}

impl EngineerState {
    /// Update decay and emission parameters from the selected phosphor type.
    pub fn sync_from_phosphor(&mut self, phosphor: &PhosphorType) {
        let layer = &phosphor.fluorescence;
        self.tau_fast = layer.tau_fast;
        self.tau_slow = layer.tau_slow;
        self.a_fast = layer.a_fast;
    }
}

const TONEMAP_MODES: &[(TonemapMode, &str)] = &[
    (TonemapMode::Reinhard, "Reinhard"),
    (TonemapMode::Aces, "ACES"),
    (TonemapMode::Clamp, "Clamp"),
    (TonemapMode::None, "None (HDR)"),
];

const SEGMENT_COLORS: &[egui::Color32] = &[
    egui::Color32::from_rgb(255, 100, 100), // Beam Write - red
    egui::Color32::from_rgb(100, 200, 100), // Decay - green
    egui::Color32::from_rgb(100, 150, 255), // Spectral Resolve - blue
    egui::Color32::from_rgb(255, 200, 80),  // Faceplate Scatter - yellow
    egui::Color32::from_rgb(200, 130, 255), // Composite - purple
];

pub fn engineer_panel(
    ui: &mut egui::Ui,
    state: &mut EngineerState,
    phosphors: &[PhosphorType],
    phosphor_index: &mut usize,
    fps: f32,
    timings: Option<&TimingHistory>,
    accum_size: Option<[u32; 2]>,
) {
    egui::ScrollArea::vertical().show(ui, |ui| {
        // -- Phosphor selector (mirrored from scope panel) --
        ui.heading("Phosphor");
        let current = &phosphors[*phosphor_index];
        egui::ComboBox::from_id_salt("eng_phosphor_type")
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

        let phosphor = &phosphors[*phosphor_index];
        emission_spectrum_plot(ui, phosphor);

        ui.separator();

        // -- Beam --
        ui.heading("Beam");
        ui.label("Core sigma");
        ui.add(egui::Slider::new(&mut state.sigma_core, 0.5..=5.0).text("px"));
        ui.label("Halo sigma");
        ui.add(egui::Slider::new(&mut state.sigma_halo, 2.0..=20.0).text("px"));
        ui.label("Halo fraction");
        ui.add(egui::Slider::new(&mut state.halo_fraction, 0.0..=0.5));
        ui.label("Space charge");
        ui.add(egui::Slider::new(&mut state.space_charge, 0.0..=1.0));
        ui.label("Accel voltage");
        ui.add(egui::Slider::new(&mut state.accel_voltage, 1.0..=25.0).text("kV"));

        ui.separator();

        // -- Decay --
        ui.heading("Decay");
        ui.label(subscript("τ", "fast"));
        ui.add(
            egui::Slider::new(&mut state.tau_fast, 0.001..=0.1)
                .logarithmic(true)
                .text("s"),
        );
        ui.label(subscript("τ", "slow"));
        ui.add(
            egui::Slider::new(&mut state.tau_slow, 0.01..=1.0)
                .logarithmic(true)
                .text("s"),
        );
        ui.label(subscript("A", "fast"));
        ui.add(egui::Slider::new(&mut state.a_fast, 0.0..=1.0));
        let mut a_slow = 1.0 - state.a_fast;
        ui.label(subscript("A", "slow"));
        if ui.add(egui::Slider::new(&mut a_slow, 0.0..=1.0)).changed() {
            state.a_fast = 1.0 - a_slow;
        }

        ui.separator();

        // -- Faceplate Scatter --
        ui.heading("Faceplate Scatter");
        ui.label("Threshold");
        ui.add(egui::Slider::new(&mut state.scatter_threshold, 0.0..=2.0));
        ui.label("Sigma");
        ui.add(egui::Slider::new(&mut state.scatter_sigma, 1.0..=16.0).text("px"));
        ui.label("Intensity");
        ui.add(egui::Slider::new(&mut state.scatter_intensity, 0.0..=1.0));

        ui.separator();

        // -- Display --
        ui.heading("Display");
        ui.label("Tonemap");
        egui::ComboBox::from_id_salt("tonemap_mode")
            .selected_text(
                TONEMAP_MODES
                    .iter()
                    .find(|(m, _)| *m == state.tonemap_mode)
                    .map_or("?", |(_, name)| name),
            )
            .show_ui(ui, |ui| {
                for &(mode, name) in TONEMAP_MODES {
                    ui.selectable_value(&mut state.tonemap_mode, mode, name);
                }
            });

        ui.label("Exposure");
        ui.add(egui::Slider::new(&mut state.exposure, 0.1..=10.0).logarithmic(true));
        ui.label("White point");
        ui.add(egui::Slider::new(&mut state.white_point, 0.1..=10.0).logarithmic(true));

        ui.separator();

        // -- Glass --
        ui.heading("Glass Faceplate");
        ui.label("Tint");
        ui.color_edit_button_rgb(&mut state.glass_tint);
        ui.label("Curvature");
        ui.add(egui::Slider::new(&mut state.curvature, 0.0..=0.5));
        ui.label("Edge Falloff");
        ui.add(egui::Slider::new(&mut state.edge_falloff, 0.0..=1.0));

        ui.separator();

        // -- Resolution --
        ui.heading("Resolution");
        ui.label("Accum buffer scale");
        ui.add(egui::Slider::new(&mut state.accum_resolution_scale, 0.25..=2.0).text("x"));
        if let Some([w, h]) = accum_size {
            ui.label(format!("{w} \u{00d7} {h}"));
        }

        ui.separator();

        // -- Stats --
        ui.heading("Stats");
        ui.label(format!("FPS: {fps:.0}"));

        if let Some(history) = timings {
            const AVG_WINDOW: usize = 32;

            ui.label(format!(
                "GPU: {} ms  |  Beam samples: {}",
                fmt_ms(history.avg_total(AVG_WINDOW)),
                history.latest_beam_samples(),
            ));

            if let Some(segs) = history.avg_segments(AVG_WINDOW) {
                for (i, (name, us)) in segs.iter().enumerate() {
                    ui.label(
                        egui::RichText::new(format!("{name}: {} ms", fmt_ms(*us)))
                            .color(SEGMENT_COLORS[i]),
                    );
                }
            }

            if history.len() > 1 {
                gpu_timing_plot(ui, history);
            }
        }
    });
}

fn subscript(base: &str, sub: &str) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    job.append(base, 0.0, egui::TextFormat::default());
    job.append(
        sub,
        0.0,
        egui::TextFormat {
            font_id: egui::FontId::proportional(9.0),
            valign: egui::Align::BOTTOM,
            ..Default::default()
        },
    );
    job
}

fn fmt_ms(us: f32) -> String {
    let ms = us / 1000.0;
    if ms < 0.01 {
        "0".into()
    } else {
        format!("{ms:.2}")
    }
}

fn emission_spectrum_plot(ui: &mut egui::Ui, phosphor: &PhosphorType) {
    use egui_plot::{Line, Plot, PlotPoints};

    use phosphor_data::spectral::{WAVELENGTH_MAX, WAVELENGTH_MIN};
    let plot = Plot::new("emission_spectrum")
        .height(100.0)
        .include_x(WAVELENGTH_MIN as f64)
        .include_x(WAVELENGTH_MAX as f64)
        .include_y(0.0)
        .set_margin_fraction(egui::Vec2::new(0.0, 0.05))
        .x_axis_label("nm")
        .show_axes([true, false])
        .allow_zoom(false)
        .allow_drag(false)
        .allow_scroll(false)
        .allow_boxed_zoom(false);

    plot.show(ui, |plot_ui| {
        let fluor_points: Vec<[f64; 2]> = (0..SPECTRAL_BANDS)
            .map(|i| {
                [
                    band_center(i) as f64,
                    phosphor.fluorescence.emission_weights[i] as f64,
                ]
            })
            .collect();
        plot_ui.line(
            Line::new("Fluorescence", PlotPoints::new(fluor_points))
                .color(egui::Color32::from_rgb(100, 255, 100))
                .allow_hover(false),
        );

        if phosphor.is_dual_layer {
            let phos_points: Vec<[f64; 2]> = (0..SPECTRAL_BANDS)
                .map(|i| {
                    [
                        band_center(i) as f64,
                        phosphor.phosphorescence.emission_weights[i] as f64,
                    ]
                })
                .collect();
            plot_ui.line(
                Line::new("Phosphorescence", PlotPoints::new(phos_points))
                    .color(egui::Color32::from_rgb(255, 200, 80))
                    .allow_hover(false),
            );
        }
    });

    if phosphor.is_dual_layer {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("Fluorescence").color(egui::Color32::from_rgb(100, 255, 100)),
            );
            ui.label(
                egui::RichText::new("Phosphorescence").color(egui::Color32::from_rgb(255, 200, 80)),
            );
        });
    }
}

fn gpu_timing_plot(ui: &mut egui::Ui, history: &TimingHistory) {
    use egui_plot::{Line, Plot, PlotPoints};

    let cap = HISTORY_CAP as f64;
    let len = history.len();
    let x_offset = (HISTORY_CAP - len) as f64;

    let plot = Plot::new("gpu_timings")
        .height(150.0)
        .include_y(0.0)
        .include_x(0.0)
        .include_x(cap - 1.0)
        .show_axes(false)
        .allow_zoom(false)
        .allow_drag(false)
        .allow_scroll(false)
        .allow_boxed_zoom(false);

    plot.show(ui, |plot_ui| {
        for seg in 0..NUM_SEGMENTS {
            let points =
                PlotPoints::from_iter(history.segment_iter(seg).map(|[x, y]| [x + x_offset, y]));
            let line = Line::new(SEGMENT_NAMES[seg], points)
                .color(SEGMENT_COLORS[seg])
                .allow_hover(false);
            plot_ui.line(line);
        }
    });
}
