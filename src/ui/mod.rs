pub mod scope_panel;

use winit::window::Window;

use crate::phosphor::{PhosphorType, phosphor_database};

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
    pub intensity: f32,
    pub focus: f32,
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

        Self {
            ctx,
            winit_state,
            phosphors: phosphor_database(),
            phosphor_index: 0,
            intensity: 1.0,
            focus: 1.5,
        }
    }

    pub fn on_event(
        &mut self,
        window: &Window,
        event: &winit::event::WindowEvent,
    ) -> egui_winit::EventResponse {
        self.winit_state.on_window_event(window, event)
    }

    pub fn run(&mut self, window: &Window) -> EguiRenderOutput {
        let raw_input = self.winit_state.take_egui_input(window);

        let full_output = self.ctx.run(raw_input, |ctx| {
            scope_panel::scope_panel(
                ctx,
                &self.phosphors,
                &mut self.phosphor_index,
                &mut self.intensity,
                &mut self.focus,
            );
        });

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

    pub fn selected_phosphor(&self) -> &PhosphorType {
        &self.phosphors[self.phosphor_index]
    }
}
