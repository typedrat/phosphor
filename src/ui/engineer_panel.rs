use std::sync::Arc;
use std::sync::atomic::Ordering;

use crate::phosphor::spectral::{SPECTRAL_BANDS, band_center};

use crate::gpu::TAU_CUTOFF;
use crate::gpu::composite::TonemapMode;
use crate::gpu::profiler::{HISTORY_CAP, NUM_SEGMENTS, SEGMENT_NAMES, TimingHistory};
use crate::phosphor::PhosphorType;
use crate::simulation_stats::SimStats;
use crate::types::Resolution;

pub struct EngineerState {
    // Beam
    pub sigma_core: f32,
    pub sigma_halo: f32,
    pub halo_fraction: f32,
    pub space_charge: f32,
    pub accel_voltage: f32,
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

/// Per-frame simulation info passed from the render loop.
pub struct SimFrameInfo {
    pub samples_this_frame: usize,
    pub sim_dt: f32,
    pub buffer_pending: usize,
}

#[allow(clippy::too_many_arguments)]
pub fn engineer_panel(
    ui: &mut egui::Ui,
    state: &mut EngineerState,
    phosphors: &[PhosphorType],
    phosphor_index: &mut usize,
    fps: f32,
    timings: Option<&TimingHistory>,
    accum_size: Option<Resolution>,
    sim_stats: Option<&Arc<SimStats>>,
    sim_frame: Option<&SimFrameInfo>,
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

        // -- Decay terms --
        ui.heading("Decay");
        decay_term_display(ui, phosphor);

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
        ui.label("Internal simulation scale");
        ui.add(
            egui::Slider::new(&mut state.accum_resolution_scale, 0.25..=4.0)
                .step_by(0.25)
                .text("x"),
        );
        if let Some(res) = accum_size {
            ui.label(res.to_string());
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
                (history.avg_beam_samples(AVG_WINDOW) / 10.0).round() as u32 * 10,
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

        // -- Simulation thread stats --
        if let Some(stats) = sim_stats {
            ui.separator();
            ui.heading("Simulation");

            let throughput = stats.throughput.load(Ordering::Relaxed);
            let batch_ms = stats.batch_interval.load(Ordering::Relaxed) * 1000.0;
            let dropped = stats.samples_dropped.load(Ordering::Relaxed);
            let capacity = stats.buffer_capacity.load(Ordering::Relaxed);

            ui.label(format!("Throughput: {throughput:.0} samples/s"));
            ui.label(format!("Batch interval: {batch_ms:.2} ms"));

            if let Some(frame) = sim_frame {
                let fill_pct = if capacity > 0 {
                    frame.buffer_pending as f32 / capacity as f32 * 100.0
                } else {
                    0.0
                };
                ui.label(format!(
                    "Buffer: {} / {} ({fill_pct:.0}%)",
                    frame.buffer_pending, capacity,
                ));
                ui.label(format!(
                    "Frame: {} samples, sim_dt={:.2} ms",
                    frame.samples_this_frame,
                    frame.sim_dt * 1000.0,
                ));
            }

            if dropped > 0 {
                ui.label(
                    egui::RichText::new(format!("Dropped: {dropped}"))
                        .color(egui::Color32::from_rgb(255, 100, 100)),
                );
            }
        }
    });
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

    use crate::phosphor::spectral::{WAVELENGTH_MAX, WAVELENGTH_MIN};
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

fn decay_term_display(ui: &mut egui::Ui, phosphor: &PhosphorType) {
    let terms = &phosphor.fluorescence.decay_terms;
    let class = phosphor_data::classify_decay_terms(terms, TAU_CUTOFF);

    ui.label(format!(
        "{} terms: {} instant, {} slow, {}",
        terms.len(),
        class.instant_exp_count,
        class.slow_exp_count,
        if class.has_power_law {
            "power-law"
        } else {
            "no power-law"
        },
    ));
    ui.label(format!("Buffer layers: {}", class.accum_layers()));

    for term in terms {
        match term {
            phosphor_data::DecayTerm::Exponential { amplitude, tau } => {
                let tier = if *tau < TAU_CUTOFF { "T1" } else { "T2" };
                ui.label(format!(
                    "  [{tier}] exp: A={amplitude:.3e}, tau={}",
                    format_time(*tau)
                ));
            }
            phosphor_data::DecayTerm::PowerLaw {
                amplitude,
                alpha,
                beta,
            } => {
                ui.label(format!(
                    "  [T3] pow: A={amplitude:.3e}, alpha={}, beta={beta:.2}",
                    format_time(*alpha)
                ));
            }
        }
    }
}

fn format_time(seconds: f32) -> String {
    if seconds >= 1.0 {
        format!("{seconds:.3} s")
    } else if seconds >= 1e-3 {
        format!("{:.3} ms", seconds * 1e3)
    } else if seconds >= 1e-6 {
        format!("{:.3} us", seconds * 1e6)
    } else {
        format!("{:.1} ns", seconds * 1e9)
    }
}
