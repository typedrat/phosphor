use strum::IntoEnumIterator;

use crate::phosphor::PhosphorType;
use crate::presets::OSCILLOSCOPE_PRESETS;
use crate::types::{ExternalMode, ExternalState, InputMode, OscilloscopeState};

use super::{AudioUiState, VectorUiState};

#[allow(clippy::too_many_arguments)]
pub fn scope_panel(
    ui: &mut egui::Ui,
    phosphors: &[PhosphorType],
    phosphor_index: &mut usize,
    intensity: &mut f32,
    focus: &mut f32,
    input_mode: &mut InputMode,
    oscilloscope: &mut OscilloscopeState,
    preset_index: &mut Option<usize>,
    audio_ui: &mut AudioUiState,
    vector_ui: &mut VectorUiState,
    external: &mut ExternalState,
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

    ui.separator();
    ui.heading("Input");

    ui.horizontal(|ui| {
        ui.selectable_value(input_mode, InputMode::Oscilloscope, "Scope");
        ui.selectable_value(input_mode, InputMode::Audio, "Audio");
        ui.selectable_value(input_mode, InputMode::Vector, "Vector");
        ui.selectable_value(input_mode, InputMode::External, "Extern");
    });

    ui.separator();

    egui::ScrollArea::vertical().show(ui, |ui| match input_mode {
        InputMode::Oscilloscope => oscilloscope_controls(ui, oscilloscope, preset_index),
        InputMode::Audio => audio_controls(ui, audio_ui),
        InputMode::Vector => vector_controls(ui, vector_ui),
        InputMode::External => external_controls(ui, external),
    });
}

fn oscilloscope_controls(
    ui: &mut egui::Ui,
    osc: &mut OscilloscopeState,
    preset_index: &mut Option<usize>,
) {
    let selected_text = match *preset_index {
        Some(i) => {
            let p = &OSCILLOSCOPE_PRESETS[i];
            format!("{} — {}", p.name, p.description)
        }
        None => "(Custom)".to_string(),
    };
    egui::ComboBox::from_id_salt("osc_preset")
        .selected_text(selected_text)
        .show_ui(ui, |ui| {
            for (i, p) in OSCILLOSCOPE_PRESETS.iter().enumerate() {
                let label = format!("{} — {}", p.name, p.description);
                if ui.selectable_value(preset_index, Some(i), label).clicked() {
                    *osc = p.state.clone();
                }
            }
        });

    ui.separator();

    // Track the state before rendering controls to detect manual changes
    let osc_before = osc.clone();

    ui.label("X Channel");
    ui.indent("x_ch", |ui| {
        egui::ComboBox::from_id_salt("x_waveform")
            .selected_text(osc.x_waveform.to_string())
            .show_ui(ui, |ui| {
                for w in crate::beam::oscilloscope::Waveform::iter() {
                    ui.selectable_value(&mut osc.x_waveform, w, w.to_string());
                }
            });
        ui.add(
            egui::Slider::new(&mut osc.x_frequency, 1.0..=10_000.0)
                .logarithmic(true)
                .text("Hz"),
        );
        ui.add(egui::Slider::new(&mut osc.x_amplitude, 0.0..=1.0).text("Amp"));
        ui.add(egui::Slider::new(&mut osc.x_phase, 0.0..=std::f32::consts::TAU).text("Phase"));
        ui.add(egui::Slider::new(&mut osc.x_dc_offset, -1.0..=1.0).text("DC"));
    });

    ui.separator();

    ui.label("Y Channel");
    ui.indent("y_ch", |ui| {
        egui::ComboBox::from_id_salt("y_waveform")
            .selected_text(osc.y_waveform.to_string())
            .show_ui(ui, |ui| {
                for w in crate::beam::oscilloscope::Waveform::iter() {
                    ui.selectable_value(&mut osc.y_waveform, w, w.to_string());
                }
            });
        ui.add(
            egui::Slider::new(&mut osc.y_frequency, 1.0..=10_000.0)
                .logarithmic(true)
                .text("Hz"),
        );
        ui.add(egui::Slider::new(&mut osc.y_amplitude, 0.0..=1.0).text("Amp"));
        ui.add(egui::Slider::new(&mut osc.y_phase, 0.0..=std::f32::consts::TAU).text("Phase"));
        ui.add(egui::Slider::new(&mut osc.y_dc_offset, -1.0..=1.0).text("DC"));
    });

    ui.separator();

    ui.add(
        egui::Slider::new(&mut osc.sample_rate, 1_000.0..=192_000.0)
            .logarithmic(true)
            .text("Sample Rate"),
    );

    // Clear preset selection if user manually changed any parameter
    if *osc != osc_before {
        *preset_index = None;
    }
}

fn audio_controls(ui: &mut egui::Ui, audio: &mut AudioUiState) {
    if ui.button("Open File...").clicked()
        && let Some(path) = rfd::FileDialog::new()
            .add_filter("Audio", &["wav", "flac", "ogg", "mp3"])
            .pick_file()
    {
        audio.pending_file = Some(path);
    }

    // Loading indicator
    if audio.decode_receiver.is_some() {
        ui.spinner();
        ui.label("Loading...");
        return;
    }

    if let Some(err) = &audio.load_error {
        ui.colored_label(egui::Color32::RED, err);
    }

    if let Some(path) = &audio.file_path
        && let Some(name) = path.file_name()
    {
        ui.label(name.to_string_lossy().as_ref());
    }

    let Some(shared) = &audio.shared else {
        return;
    };

    use std::sync::atomic::Ordering;

    ui.separator();

    // Play/Pause + Loop
    ui.horizontal(|ui| {
        let playing = shared.playing.load(Ordering::Relaxed);
        let play_label = if playing { "Pause" } else { "Play" };
        if ui.button(play_label).clicked() {
            shared.playing.store(!playing, Ordering::Relaxed);
        }
        let mut looping = shared.looping.load(Ordering::Relaxed);
        if ui.checkbox(&mut looping, "Loop").changed() {
            shared.looping.store(looping, Ordering::Relaxed);
        }
    });

    // Seek bar
    let duration = shared.duration_secs();
    let mut pos_secs = shared.position_secs();
    let seek_bar = egui::Slider::new(&mut pos_secs, 0.0..=duration)
        .show_value(false)
        .trailing_fill(true);
    if ui.add(seek_bar).changed() {
        shared.seek_to_secs(pos_secs);
    }

    // Position / Duration label
    let pos_display = format_time(pos_secs);
    let dur_display = format_time(duration);
    ui.label(format!("{pos_display} / {dur_display}"));

    // Speed
    let mut speed = shared.speed.load(Ordering::Relaxed);
    if ui
        .add(
            egui::Slider::new(&mut speed, 0.25..=4.0)
                .logarithmic(true)
                .text("Speed"),
        )
        .changed()
    {
        shared.speed.store(speed, Ordering::Relaxed);
    }

    // Volume
    let mut volume = shared.volume.load(Ordering::Relaxed);
    if ui
        .add(egui::Slider::new(&mut volume, 0.0..=1.0).text("Volume"))
        .changed()
    {
        shared.volume.store(volume, Ordering::Relaxed);
    }
}

pub fn format_time(secs: f32) -> String {
    let total = secs as u32;
    let m = total / 60;
    let s = total % 60;
    format!("{m}:{s:02}")
}

fn vector_controls(ui: &mut egui::Ui, vector: &mut VectorUiState) {
    if ui.button("Open File...").clicked()
        && let Some(path) = rfd::FileDialog::new()
            .add_filter("JSON", &["json"])
            .pick_file()
    {
        vector.pending_file = Some(path);
    }

    if let Some(err) = &vector.load_error {
        ui.colored_label(egui::Color32::RED, err);
    }

    if let Some(path) = &vector.file_path
        && let Some(name) = path.file_name()
    {
        ui.label(name.to_string_lossy().as_ref());
    }

    if vector.segment_count > 0 {
        ui.label(format!("{} segments", vector.segment_count));

        ui.separator();

        ui.add(
            egui::Slider::new(&mut vector.beam_speed, 0.1..=10.0)
                .logarithmic(true)
                .text("Beam Speed"),
        );
        ui.add(
            egui::Slider::new(&mut vector.settling_time, 0.0001..=0.01)
                .logarithmic(true)
                .text("Settling"),
        );
        ui.checkbox(&mut vector.looping, "Loop");
    }
}

fn external_controls(ui: &mut egui::Ui, external: &mut ExternalState) {
    ui.horizontal(|ui| {
        ui.selectable_value(&mut external.mode, ExternalMode::Stdin, "stdin");
        ui.selectable_value(&mut external.mode, ExternalMode::Socket, "Socket");
    });

    if external.mode == ExternalMode::Socket {
        ui.horizontal(|ui| {
            ui.label("Path:");
            ui.text_edit_singleline(&mut external.socket_path);
        });
    }

    let (color, text) = if external.connected {
        (egui::Color32::GREEN, "Connected")
    } else {
        (egui::Color32::GRAY, "Not connected")
    };
    ui.colored_label(color, text);

    ui.label("Not yet implemented");
}
