use strum::IntoEnumIterator;

use crate::app::{ExternalMode, InputMode, InputState};
use crate::phosphor::PhosphorType;

pub fn scope_panel(
    ui: &mut egui::Ui,
    phosphors: &[PhosphorType],
    phosphor_index: &mut usize,
    intensity: &mut f32,
    focus: &mut f32,
    input: &mut InputState,
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
        ui.selectable_value(&mut input.mode, InputMode::Oscilloscope, "Scope");
        ui.selectable_value(&mut input.mode, InputMode::Audio, "Audio");
        ui.selectable_value(&mut input.mode, InputMode::Vector, "Vector");
        ui.selectable_value(&mut input.mode, InputMode::External, "Extern");
    });

    ui.separator();

    egui::ScrollArea::vertical().show(ui, |ui| match input.mode {
        InputMode::Oscilloscope => oscilloscope_controls(ui, input),
        InputMode::Audio => audio_controls(ui, input),
        InputMode::Vector => vector_controls(ui, input),
        InputMode::External => external_controls(ui, input),
    });
}

fn oscilloscope_controls(ui: &mut egui::Ui, input: &mut InputState) {
    let osc = &mut input.oscilloscope;

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
}

fn audio_controls(ui: &mut egui::Ui, input: &mut InputState) {
    if ui.button("Open File...").clicked()
        && let Some(path) = rfd::FileDialog::new()
            .add_filter("Audio", &["wav", "flac", "ogg", "mp3"])
            .pick_file()
    {
        input.load_audio_file(path);
    }

    if let Some(err) = &input.audio.load_error {
        ui.colored_label(egui::Color32::RED, err);
    }

    if let Some(path) = &input.audio.file_path
        && let Some(name) = path.file_name()
    {
        ui.label(name.to_string_lossy().as_ref());
    }

    if input.audio.source.is_some() {
        ui.separator();
        ui.horizontal(|ui| {
            let play_label = if input.audio.playing { "Pause" } else { "Play" };
            if ui.button(play_label).clicked() {
                input.audio.playing = !input.audio.playing;
            }
            if ui.button("Stop").clicked() {
                input.audio.playing = false;
                if let Some(source) = &mut input.audio.source {
                    source.seek(0.0);
                }
            }
            ui.checkbox(&mut input.audio.looping, "Loop");
        });

        if let Some(source) = &mut input.audio.source {
            let duration = source.duration_secs();
            if duration > 0.0 {
                let mut frac = source.position_secs() / duration;
                if ui
                    .add(egui::Slider::new(&mut frac, 0.0..=1.0).text("Seek"))
                    .changed()
                {
                    source.seek(frac);
                }
            }
        }

        ui.add(
            egui::Slider::new(&mut input.audio.speed, 0.25..=4.0)
                .logarithmic(true)
                .text("Speed"),
        );
    }
}

fn vector_controls(ui: &mut egui::Ui, input: &mut InputState) {
    if ui.button("Open File...").clicked()
        && let Some(path) = rfd::FileDialog::new()
            .add_filter("JSON", &["json"])
            .pick_file()
    {
        input.load_vector_file(path);
    }

    if let Some(err) = &input.vector.load_error {
        ui.colored_label(egui::Color32::RED, err);
    }

    if let Some(path) = &input.vector.file_path
        && let Some(name) = path.file_name()
    {
        ui.label(name.to_string_lossy().as_ref());
    }

    if !input.vector.segments.is_empty() {
        ui.label(format!("{} segments", input.vector.segments.len()));

        ui.separator();

        ui.add(
            egui::Slider::new(&mut input.vector.beam_speed, 0.1..=10.0)
                .logarithmic(true)
                .text("Beam Speed"),
        );
        ui.add(
            egui::Slider::new(&mut input.vector.settling_time, 0.0001..=0.01)
                .logarithmic(true)
                .text("Settling"),
        );
        ui.checkbox(&mut input.vector.looping, "Loop");
    }
}

fn external_controls(ui: &mut egui::Ui, input: &mut InputState) {
    ui.horizontal(|ui| {
        ui.selectable_value(&mut input.external.mode, ExternalMode::Stdin, "stdin");
        ui.selectable_value(&mut input.external.mode, ExternalMode::Socket, "Socket");
    });

    if input.external.mode == ExternalMode::Socket {
        ui.horizontal(|ui| {
            ui.label("Path:");
            ui.text_edit_singleline(&mut input.external.socket_path);
        });
    }

    let (color, text) = if input.external.connected {
        (egui::Color32::GREEN, "Connected")
    } else {
        (egui::Color32::GRAY, "Not connected")
    };
    ui.colored_label(color, text);

    ui.label("Not yet implemented");
}
