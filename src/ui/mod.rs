pub mod engineer_panel;
pub mod scope_panel;

use winit::window::Window;

use crate::app::InputState;
use crate::gpu::profiler::TimingHistory;
use crate::phosphor::{PhosphorType, phosphor_database};

pub use engineer_panel::EngineerState;

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

pub struct UiState {
    pub ctx: egui::Context,
    winit_state: egui_winit::State,
    pub phosphors: Vec<PhosphorType>,
    pub phosphor_index: usize,
    prev_phosphor_index: usize,
    pub intensity: f32,
    pub focus: f32,
    pub engineer: EngineerState,
    pub input: InputState,
    tab: PanelTab,
    pub panel_visible: bool,
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
        let mut engineer = EngineerState::default();
        engineer.sync_from_phosphor(&phosphors[0]);

        Self {
            ctx,
            winit_state,
            phosphors,
            phosphor_index: 0,
            prev_phosphor_index: 0,
            intensity: 1.0,
            focus: 1.5,
            engineer,
            input: InputState::default(),
            tab: PanelTab::default(),
            panel_visible: true,
        }
    }

    pub fn on_event(
        &mut self,
        window: &Window,
        event: &winit::event::WindowEvent,
    ) -> egui_winit::EventResponse {
        self.winit_state.on_window_event(window, event)
    }

    pub fn run(&mut self, window: &Window, timings: Option<&TimingHistory>) -> EguiRenderOutput {
        let raw_input = self.winit_state.take_egui_input(window);
        let fps = 1.0 / self.ctx.input(|i| i.predicted_dt);

        let full_output = self.ctx.run(raw_input, |ctx| {
            if self.panel_visible {
                egui::SidePanel::left("control_panel")
                    .default_width(220.0)
                    .show(ctx, |ui| {
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

                        match self.tab {
                            PanelTab::Scope => scope_panel::scope_panel(
                                ui,
                                &self.phosphors,
                                &mut self.phosphor_index,
                                &mut self.intensity,
                                &mut self.focus,
                                &mut self.input,
                            ),
                            PanelTab::Engineer => {
                                engineer_panel::engineer_panel(
                                    ui,
                                    &mut self.engineer,
                                    &self.phosphors,
                                    &mut self.phosphor_index,
                                    fps,
                                    timings,
                                );
                            }
                        }
                    });
            } else {
                egui::Area::new(egui::Id::new("panel_toggle"))
                    .fixed_pos(egui::pos2(8.0, 8.0))
                    .show(ctx, |ui| {
                        if ui.button("\u{2630}").clicked() {
                            self.panel_visible = true;
                        }
                    });
            }
        });

        // Sync engineer params when phosphor selection changes
        if self.phosphor_index != self.prev_phosphor_index {
            self.engineer
                .sync_from_phosphor(&self.phosphors[self.phosphor_index]);
            self.prev_phosphor_index = self.phosphor_index;
        }

        self.winit_state
            .handle_platform_output(window, full_output.platform_output);

        let primitives = self
            .ctx
            .tessellate(full_output.shapes, full_output.pixels_per_point);

        let size = window.inner_size();

        EguiRenderOutput {
            primitives,
            textures_delta: full_output.textures_delta,
            screen_descriptor: egui_wgpu::ScreenDescriptor {
                size_in_pixels: [size.width, size.height],
                pixels_per_point: full_output.pixels_per_point,
            },
        }
    }

    pub fn run_detached(
        &mut self,
        window: &Window,
        egui_winit: &mut egui_winit::State,
        timings: Option<&TimingHistory>,
    ) -> EguiRenderOutput {
        let raw_input = egui_winit.take_egui_input(window);
        let fps = 1.0 / self.ctx.input(|i| i.predicted_dt);

        let full_output = self.ctx.run(raw_input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.tab, PanelTab::Scope, "Scope");
                    ui.selectable_value(&mut self.tab, PanelTab::Engineer, "Engineer");
                });
                ui.separator();

                match self.tab {
                    PanelTab::Scope => scope_panel::scope_panel(
                        ui,
                        &self.phosphors,
                        &mut self.phosphor_index,
                        &mut self.intensity,
                        &mut self.focus,
                        &mut self.input,
                    ),
                    PanelTab::Engineer => {
                        engineer_panel::engineer_panel(
                            ui,
                            &mut self.engineer,
                            &self.phosphors,
                            &mut self.phosphor_index,
                            fps,
                            timings,
                        );
                    }
                }
            });
        });

        if self.phosphor_index != self.prev_phosphor_index {
            self.engineer
                .sync_from_phosphor(&self.phosphors[self.phosphor_index]);
            self.prev_phosphor_index = self.phosphor_index;
        }

        egui_winit.handle_platform_output(window, full_output.platform_output);

        let primitives = self
            .ctx
            .tessellate(full_output.shapes, full_output.pixels_per_point);

        let size = window.inner_size();

        EguiRenderOutput {
            primitives,
            textures_delta: full_output.textures_delta,
            screen_descriptor: egui_wgpu::ScreenDescriptor {
                size_in_pixels: [size.width, size.height],
                pixels_per_point: full_output.pixels_per_point,
            },
        }
    }

    pub fn selected_phosphor(&self) -> &PhosphorType {
        &self.phosphors[self.phosphor_index]
    }
}
