use std::path::PathBuf;
use std::time::Duration;

use crate::types::InputMode;

// ---------------------------------------------------------------------------
// ResolutionPreset
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ResolutionPreset {
    R720p,
    #[default]
    R1080p,
    R1440p,
    R4k,
    R8k,
    Custom,
}

impl ResolutionPreset {
    pub fn dimensions(self) -> Option<(u32, u32)> {
        match self {
            Self::R720p => Some((1280, 720)),
            Self::R1080p => Some((1920, 1080)),
            Self::R1440p => Some((2560, 1440)),
            Self::R4k => Some((3840, 2160)),
            Self::R8k => Some((7680, 4320)),
            Self::Custom => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::R720p => "720p",
            Self::R1080p => "1080p",
            Self::R1440p => "1440p",
            Self::R4k => "4K",
            Self::R8k => "8K",
            Self::Custom => "Custom",
        }
    }
}

// ---------------------------------------------------------------------------
// FpsPreset
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FpsPreset {
    F24,
    F30,
    #[default]
    F60,
    F120,
    Custom,
}

impl FpsPreset {
    pub fn value(self) -> Option<u32> {
        match self {
            Self::F24 => Some(24),
            Self::F30 => Some(30),
            Self::F60 => Some(60),
            Self::F120 => Some(120),
            Self::Custom => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::F24 => "24 fps",
            Self::F30 => "30 fps",
            Self::F60 => "60 fps",
            Self::F120 => "120 fps",
            Self::Custom => "Custom",
        }
    }
}

// ---------------------------------------------------------------------------
// RecordingProgress
// ---------------------------------------------------------------------------

pub struct RecordingProgress {
    pub current_frame: u64,
    pub total_frames: u64,
    pub elapsed: Duration,
    pub pre_rolling: bool,
}

// ---------------------------------------------------------------------------
// RecordingUiState — defined here, re-exported from mod.rs
// ---------------------------------------------------------------------------

pub struct RecordingUiState {
    pub resolution_preset: ResolutionPreset,
    pub custom_width: u32,
    pub custom_height: u32,
    pub fps_preset: FpsPreset,
    pub custom_fps: u32,
    pub duration_secs: f32,
    pub output_path: Option<PathBuf>,
    pub encoding_preset: crate::recording::ffmpeg::EncodingPreset,
    pub custom_ffmpeg_args: String,
    pub recording_progress: Option<RecordingProgress>,
    pub start_requested: bool,
    pub cancel_requested: bool,
}

impl Default for RecordingUiState {
    fn default() -> Self {
        Self {
            resolution_preset: ResolutionPreset::default(),
            custom_width: 1920,
            custom_height: 1080,
            fps_preset: FpsPreset::default(),
            custom_fps: 60,
            duration_secs: 10.0,
            output_path: None,
            encoding_preset: crate::recording::ffmpeg::EncodingPreset::default(),
            custom_ffmpeg_args: String::new(),
            recording_progress: None,
            start_requested: false,
            cancel_requested: false,
        }
    }
}

impl RecordingUiState {
    pub fn effective_resolution(&self, max_dim: u32) -> (u32, u32) {
        let (w, h) = match self.resolution_preset.dimensions() {
            Some(dims) => dims,
            None => (self.custom_width, self.custom_height),
        };
        let w = w.min(max_dim);
        let h = h.min(max_dim);
        (w, h)
    }

    pub fn effective_fps(&self) -> u32 {
        match self.fps_preset.value() {
            Some(fps) => fps,
            None => self.custom_fps,
        }
    }

    pub fn is_recording(&self) -> bool {
        self.recording_progress.is_some()
    }
}

// ---------------------------------------------------------------------------
// Panel function
// ---------------------------------------------------------------------------

pub fn recording_panel(ui: &mut egui::Ui, state: &mut RecordingUiState, input_mode: InputMode) {
    egui::ScrollArea::vertical().show(ui, |ui| {
        // ----------------------------------------------------------------
        // Output
        // ----------------------------------------------------------------
        ui.heading("Output");

        // Resolution preset
        ui.label("Resolution");
        egui::ComboBox::from_id_salt("rec_resolution_preset")
            .selected_text(state.resolution_preset.label())
            .show_ui(ui, |ui| {
                use ResolutionPreset::*;
                for preset in [R720p, R1080p, R1440p, R4k, R8k, Custom] {
                    ui.selectable_value(&mut state.resolution_preset, preset, preset.label());
                }
            });

        if state.resolution_preset == ResolutionPreset::Custom {
            ui.horizontal(|ui| {
                ui.label("W");
                ui.add(egui::DragValue::new(&mut state.custom_width).range(1..=16384));
                ui.label("H");
                ui.add(egui::DragValue::new(&mut state.custom_height).range(1..=16384));
            });
        }

        // FPS preset
        ui.label("Frame Rate");
        egui::ComboBox::from_id_salt("rec_fps_preset")
            .selected_text(state.fps_preset.label())
            .show_ui(ui, |ui| {
                use FpsPreset::*;
                for preset in [F24, F30, F60, F120, Custom] {
                    ui.selectable_value(&mut state.fps_preset, preset, preset.label());
                }
            });

        if state.fps_preset == FpsPreset::Custom {
            ui.horizontal(|ui| {
                ui.label("fps");
                ui.add(egui::DragValue::new(&mut state.custom_fps).range(1..=1000));
            });
        }

        // Duration — only shown for non-audio modes
        if input_mode != InputMode::Audio {
            ui.label("Duration (s)");
            ui.add(
                egui::DragValue::new(&mut state.duration_secs)
                    .range(0.1..=3600.0)
                    .speed(0.1),
            );
        }

        // Output path
        ui.label("Output File");
        ui.horizontal(|ui| {
            let path_text = state
                .output_path
                .as_ref()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "(none)".to_string());
            ui.label(path_text);
            if ui.button("Browse…").clicked() {
                let ext = state.encoding_preset.extension();
                let filter_name = ext.to_uppercase();
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter(&filter_name, &[ext])
                    .save_file()
                {
                    state.output_path = Some(path);
                }
            }
        });

        ui.separator();

        // ----------------------------------------------------------------
        // Encoding
        // ----------------------------------------------------------------
        ui.heading("Encoding");

        ui.label("Preset");
        egui::ComboBox::from_id_salt("rec_encoding_preset")
            .selected_text(state.encoding_preset.description())
            .show_ui(ui, |ui| {
                for &preset in crate::recording::ffmpeg::EncodingPreset::ALL {
                    ui.selectable_value(&mut state.encoding_preset, preset, preset.description());
                }
            });

        ui.label("Custom ffmpeg args");
        ui.text_edit_singleline(&mut state.custom_ffmpeg_args);

        ui.separator();

        // ----------------------------------------------------------------
        // Controls
        // ----------------------------------------------------------------
        ui.heading("Controls");

        if let Some(progress) = &state.recording_progress {
            let pct = if progress.total_frames > 0 {
                progress.current_frame as f32 / progress.total_frames as f32
            } else {
                0.0
            };

            if progress.pre_rolling {
                ui.label("Pre-rolling…");
            } else {
                ui.label(format!(
                    "Frame {}/{}  ({:.0}%)",
                    progress.current_frame,
                    progress.total_frames,
                    pct * 100.0
                ));
            }

            ui.add(egui::ProgressBar::new(pct).show_percentage());

            let elapsed = progress.elapsed.as_secs_f32();
            ui.label(format!("Elapsed: {:.1}s", elapsed));

            if ui.button("Cancel").clicked() {
                state.cancel_requested = true;
            }
        } else {
            let can_record = state.output_path.is_some();
            let record_btn = egui::Button::new("Record");
            if ui.add_enabled(can_record, record_btn).clicked() {
                state.start_requested = true;
            }
            if !can_record {
                ui.label(
                    egui::RichText::new("Choose an output file to record.")
                        .color(egui::Color32::GRAY),
                );
            }
        }
    });
}
