pub mod engineer_panel;
pub mod media_overlay;
pub mod scope_panel;

use std::path::PathBuf;
use std::sync::Arc;

use winit::window::Window;

use crate::audio_output::SharedAudioPlayback;
use crate::gpu::profiler::TimingHistory;
use crate::phosphor::{PhosphorType, phosphor_database};
use crate::simulation_stats::SimStats;
use crate::types::Resolution;
use crate::types::{ExternalState, InputMode, OscilloscopeState};

pub use engineer_panel::EngineerState;
pub use engineer_panel::SimFrameInfo;

#[derive(Default, PartialEq)]
pub enum PanelTab {
    #[default]
    Scope,
    Engineer,
}

pub struct EguiRenderOutput {
    pub primitives: Vec<egui::ClippedPrimitive>,
    pub textures_delta: egui::TexturesDelta,
    pub screen_descriptor: egui_wgpu::ScreenDescriptor,
}

/// UI-only audio state. Playback controls (playing, looping, speed, volume)
/// are read/written directly on SharedAudioPlayback atomics.
#[derive(Default)]
pub struct AudioUiState {
    pub file_path: Option<PathBuf>,
    pub shared: Option<Arc<SharedAudioPlayback>>,
    pub decode_receiver:
        Option<crossbeam_channel::Receiver<anyhow::Result<crate::audio_output::DecodedAudio>>>,
    pub load_error: Option<String>,
    pub pending_file: Option<PathBuf>,
}

/// UI-only vector state (segment data lives on the sim thread).
pub struct VectorUiState {
    pub file_path: Option<PathBuf>,
    pub segment_count: usize,
    pub beam_speed: f32,
    pub settling_time: f32,
    pub looping: bool,
    pub load_error: Option<String>,
    /// Set by the UI when a file is picked; consumed by the render thread.
    pub pending_file: Option<PathBuf>,
}

impl Default for VectorUiState {
    fn default() -> Self {
        Self {
            file_path: None,
            segment_count: 0,
            beam_speed: 1.0,
            settling_time: 0.001,
            looping: true,
            load_error: None,
            pending_file: None,
        }
    }
}

pub struct UiState {
    pub ctx: egui::Context,
    winit_state: egui_winit::State,
    pub phosphors: Vec<PhosphorType>,
    pub phosphor_index: usize,
    prev_phosphor_index: usize,
    pub intensity: f32,
    pub focus: f32,
    pub engineer: EngineerState,
    // UI-local copies of input state (sim thread owns the real InputState)
    pub input_mode: InputMode,
    pub oscilloscope: OscilloscopeState,
    pub audio_ui: AudioUiState,
    pub vector_ui: VectorUiState,
    pub external: ExternalState,
    pub preset_index: Option<usize>,
    tab: PanelTab,
    pub panel_visible: bool,
    pub panel_width: f32,
    pub accum_size: Option<Resolution>,
    pub media_overlay: media_overlay::MediaOverlay,
}

impl UiState {
    pub fn new(window: &Window) -> Self {
        let ctx = egui::Context::default();
        let winit_state = egui_winit::State::new(
            ctx.clone(),
            egui::ViewportId::ROOT,
            window,
            Some(window.scale_factor() as f32),
            window.theme(),
            None,
        );

        let phosphors = phosphor_database();
        let engineer = EngineerState::default();

        Self {
            ctx,
            winit_state,
            phosphors,
            phosphor_index: 0,
            prev_phosphor_index: 0,
            intensity: 1.0,
            focus: 1.5,
            engineer,
            input_mode: InputMode::default(),
            oscilloscope: OscilloscopeState::default(),
            audio_ui: AudioUiState::default(),
            vector_ui: VectorUiState::default(),
            external: ExternalState::default(),
            preset_index: Some(0),
            tab: PanelTab::default(),
            panel_visible: true,
            panel_width: 0.0,
            accum_size: None,
            media_overlay: media_overlay::MediaOverlay::default(),
        }
    }

    pub fn on_event(
        &mut self,
        window: &Window,
        event: &winit::event::WindowEvent,
    ) -> egui_winit::EventResponse {
        self.winit_state.on_window_event(window, event)
    }

    pub fn run(
        &mut self,
        window: &Window,
        timings: Option<&TimingHistory>,
        sim_stats: Option<&Arc<SimStats>>,
        sim_frame: Option<&SimFrameInfo>,
    ) -> EguiRenderOutput {
        let raw_input = self.winit_state.take_egui_input(window);
        let ctx = self.ctx.clone();
        let fps = 1.0 / ctx.input(|i| i.predicted_dt);

        let full_output = ctx.run(raw_input, |egui_ctx| {
            if self.panel_visible {
                let panel_response = egui::SidePanel::left("control_panel")
                    .default_width(220.0)
                    .show(egui_ctx, |ui| {
                        ui.horizontal(|ui| {
                            ui.selectable_value(&mut self.tab, PanelTab::Scope, "Scope");
                            ui.selectable_value(&mut self.tab, PanelTab::Engineer, "Engineer");
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui.small_button("\u{00d7}").clicked() {
                                        self.panel_visible = false;
                                    }
                                },
                            );
                        });
                        ui.separator();
                        self.draw_panels(ui, fps, timings, sim_stats, sim_frame);
                    });
                self.panel_width = panel_response.response.rect.width();
            } else {
                self.panel_width = 0.0;
                egui::Area::new(egui::Id::new("panel_toggle"))
                    .fixed_pos(egui::pos2(8.0, 8.0))
                    .show(egui_ctx, |ui| {
                        if ui.button("\u{2630}").clicked() {
                            self.panel_visible = true;
                        }
                    });
            }

            // Media overlay — viewport is the area not covered by the panel
            let screen = egui_ctx.viewport_rect();
            let viewport = egui::Rect::from_min_size(
                egui::pos2(self.panel_width, 0.0),
                egui::vec2(screen.width() - self.panel_width, screen.height()),
            );
            self.media_overlay.show(
                egui_ctx,
                viewport,
                self.input_mode,
                self.audio_ui.shared.as_ref(),
            );
        });

        let egui::FullOutput {
            platform_output,
            shapes,
            pixels_per_point,
            textures_delta,
            ..
        } = full_output;
        self.winit_state
            .handle_platform_output(window, platform_output);
        tessellate_output(&self.ctx, window, shapes, pixels_per_point, textures_delta)
    }

    pub fn run_detached(
        &mut self,
        window: &Window,
        egui_winit: &mut egui_winit::State,
        timings: Option<&TimingHistory>,
        sim_stats: Option<&Arc<SimStats>>,
        sim_frame: Option<&SimFrameInfo>,
    ) -> EguiRenderOutput {
        let raw_input = egui_winit.take_egui_input(window);
        let ctx = self.ctx.clone();
        let fps = 1.0 / ctx.input(|i| i.predicted_dt);

        let full_output = ctx.run(raw_input, |egui_ctx| {
            egui::CentralPanel::default().show(egui_ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.tab, PanelTab::Scope, "Scope");
                    ui.selectable_value(&mut self.tab, PanelTab::Engineer, "Engineer");
                });
                ui.separator();
                self.draw_panels(ui, fps, timings, sim_stats, sim_frame);
            });
        });

        let egui::FullOutput {
            platform_output,
            shapes,
            pixels_per_point,
            textures_delta,
            ..
        } = full_output;
        egui_winit.handle_platform_output(window, platform_output);
        tessellate_output(&self.ctx, window, shapes, pixels_per_point, textures_delta)
    }

    fn draw_panels(
        &mut self,
        ui: &mut egui::Ui,
        fps: f32,
        timings: Option<&TimingHistory>,
        sim_stats: Option<&Arc<SimStats>>,
        sim_frame: Option<&SimFrameInfo>,
    ) {
        match self.tab {
            PanelTab::Scope => {
                scope_panel::scope_panel(
                    ui,
                    &self.phosphors,
                    &mut self.phosphor_index,
                    &mut self.intensity,
                    &mut self.focus,
                    &mut self.input_mode,
                    &mut self.oscilloscope,
                    &mut self.preset_index,
                    &mut self.audio_ui,
                    &mut self.vector_ui,
                    &mut self.external,
                );
            }
            PanelTab::Engineer => {
                engineer_panel::engineer_panel(
                    ui,
                    &mut self.engineer,
                    &self.phosphors,
                    &mut self.phosphor_index,
                    fps,
                    timings,
                    self.accum_size,
                    sim_stats,
                    sim_frame,
                );
            }
        }
    }

    pub fn selected_phosphor(&self) -> &PhosphorType {
        &self.phosphors[self.phosphor_index]
    }

    /// Returns true once per phosphor selection change, consuming the event.
    pub fn phosphor_changed(&mut self) -> bool {
        if self.phosphor_index != self.prev_phosphor_index {
            self.prev_phosphor_index = self.phosphor_index;
            true
        } else {
            false
        }
    }
}

fn tessellate_output(
    ctx: &egui::Context,
    window: &Window,
    shapes: Vec<egui::epaint::ClippedShape>,
    pixels_per_point: f32,
    textures_delta: egui::TexturesDelta,
) -> EguiRenderOutput {
    let primitives = ctx.tessellate(shapes, pixels_per_point);
    let size = window.inner_size();
    EguiRenderOutput {
        primitives,
        textures_delta,
        screen_descriptor: egui_wgpu::ScreenDescriptor {
            size_in_pixels: [size.width, size.height],
            pixels_per_point,
        },
    }
}
