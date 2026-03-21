use std::sync::Arc;

use winit::event::WindowEvent;
use winit::window::Window;

use crate::audio_output::SharedAudioPlayback;
use crate::types::InputMode;

use super::EguiRenderOutput;

/// Floating media overlay with its own egui context and renderer.
///
/// Uses a dedicated `egui::Context` to avoid conflicts with the shared
/// context used for the sidebar panel — egui doesn't support two
/// `ctx.run()` calls per frame on the same context with interactive widgets.
pub struct MediaOverlay {
    opacity: f32,
    fade_target: f32,
    ctx: egui::Context,
    winit_state: Option<egui_winit::State>,
    pub renderer: Option<egui_wgpu::Renderer>,
}

impl Default for MediaOverlay {
    fn default() -> Self {
        Self {
            opacity: 0.0,
            fade_target: 0.0,
            ctx: egui::Context::default(),
            winit_state: None,
            renderer: None,
        }
    }
}

impl MediaOverlay {
    /// Initialize the overlay's egui pipeline. Must be called after the
    /// window and GPU device are available.
    pub fn init(&mut self, window: &Window, device: &wgpu::Device, format: wgpu::TextureFormat) {
        self.winit_state = Some(egui_winit::State::new(
            self.ctx.clone(),
            egui::ViewportId::from_hash_of("media_overlay"),
            window,
            Some(window.scale_factor() as f32),
            window.theme(),
            None,
        ));
        self.renderer = Some(egui_wgpu::Renderer::new(device, format, Default::default()));
    }

    /// Forward a window event to the overlay's egui input handler.
    pub fn on_event(&mut self, window: &Window, event: &WindowEvent) {
        if let Some(state) = &mut self.winit_state {
            let _ = state.on_window_event(window, event);
        }
    }

    /// Run the overlay's egui pass. Returns render output if there's
    /// something to draw, `None` otherwise.
    pub fn run(
        &mut self,
        window: &Window,
        viewport_rect: egui::Rect,
        input_mode: InputMode,
        shared: Option<&Arc<SharedAudioPlayback>>,
    ) -> Option<EguiRenderOutput> {
        let Some(winit_state) = &mut self.winit_state else {
            return None;
        };

        let raw_input = winit_state.take_egui_input(window);
        let ctx = self.ctx.clone();

        // Pull out fields needed by the closure to avoid borrowing all of `self`
        let opacity = &mut self.opacity;
        let fade_target = &mut self.fade_target;

        let full_output = ctx.run(raw_input, |egui_ctx| {
            Self::show_inner(
                opacity,
                fade_target,
                egui_ctx,
                viewport_rect,
                input_mode,
                shared,
            );
        });

        let egui::FullOutput {
            platform_output,
            shapes,
            pixels_per_point,
            textures_delta,
            ..
        } = full_output;
        winit_state.handle_platform_output(window, platform_output);

        let primitives = ctx.tessellate(shapes, pixels_per_point);

        // Skip rendering if there are no primitives (overlay not visible)
        if primitives.is_empty() {
            return None;
        }

        let size = window.inner_size();
        Some(EguiRenderOutput {
            primitives,
            textures_delta,
            screen_descriptor: egui_wgpu::ScreenDescriptor {
                size_in_pixels: [size.width, size.height],
                pixels_per_point,
            },
        })
    }

    fn show_inner(
        opacity: &mut f32,
        fade_target: &mut f32,
        ctx: &egui::Context,
        viewport_rect: egui::Rect,
        input_mode: InputMode,
        shared: Option<&Arc<SharedAudioPlayback>>,
    ) {
        let Some(shared) = shared else {
            *opacity = 0.0;
            return;
        };
        if input_mode != InputMode::Audio {
            *opacity = 0.0;
            return;
        }

        let overlay_height = 44.0;
        let overlay_width = viewport_rect.width() * 0.8;
        let overlay_rect = egui::Rect::from_center_size(
            egui::pos2(
                viewport_rect.center().x,
                viewport_rect.bottom() - overlay_height / 2.0 - 12.0,
            ),
            egui::vec2(overlay_width, overlay_height),
        );

        let pointer_pos = ctx.input(|i| i.pointer.hover_pos());
        let hovering =
            pointer_pos.is_some_and(|p| viewport_rect.contains(p) || overlay_rect.contains(p));

        // Animate opacity
        *fade_target = if hovering { 1.0 } else { 0.0 };
        let dt = ctx.input(|i| i.predicted_dt);
        let fade_speed = 1.0 / 0.2; // 200ms fade
        if *opacity < *fade_target {
            *opacity = (*opacity + dt * fade_speed).min(*fade_target);
        } else {
            *opacity = (*opacity - dt * fade_speed).max(*fade_target);
        }

        if (*opacity - *fade_target).abs() > 0.01 {
            ctx.request_repaint();
        }

        if *opacity < 0.01 {
            return;
        }

        let alpha = (*opacity * 255.0) as u8;
        let fill_alpha = (180.0 * *opacity) as u8;

        egui::Area::new(egui::Id::new("media_overlay"))
            .fixed_pos(overlay_rect.left_top())
            .show(ctx, |ui| {
                let frame = egui::Frame::new()
                    .fill(egui::Color32::from_black_alpha(fill_alpha))
                    .corner_radius(8.0)
                    .inner_margin(8.0);

                frame.show(ui, |ui| {
                    ui.set_min_width(overlay_width - 16.0);
                    ui.set_max_width(overlay_width - 16.0);

                    ui.visuals_mut().override_text_color =
                        Some(egui::Color32::from_white_alpha(alpha));

                    ui.horizontal(|ui| {
                        use std::sync::atomic::Ordering;

                        let playing = shared.playing.load(Ordering::Relaxed);
                        let label = if playing { "\u{23F8}" } else { "\u{25B6}" };
                        if ui.button(label).clicked() {
                            shared.playing.store(!playing, Ordering::Relaxed);
                        }

                        let duration = shared.duration_secs();
                        let mut pos = shared.position_secs();
                        let slider = egui::Slider::new(&mut pos, 0.0..=duration)
                            .show_value(false)
                            .trailing_fill(true);
                        if ui.add(slider).changed() {
                            shared.seek_to_secs(pos);
                        }

                        let pos_str = super::scope_panel::format_time(pos);
                        let dur_str = super::scope_panel::format_time(duration);
                        ui.label(format!("{pos_str} / {dur_str}"));
                    });
                });
            });
    }
}
