use std::sync::Arc;

use crate::audio_output::SharedAudioPlayback;
use crate::types::InputMode;

pub struct MediaOverlay {
    opacity: f32,
    fade_target: f32,
}

impl Default for MediaOverlay {
    fn default() -> Self {
        Self {
            opacity: 0.0,
            fade_target: 0.0,
        }
    }
}

impl MediaOverlay {
    /// Show the media overlay if appropriate.
    ///
    /// `viewport_rect`: the screen area where the overlay should appear
    /// (full window in detached mode, right-of-sidebar in combined mode).
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        viewport_rect: egui::Rect,
        input_mode: InputMode,
        shared: Option<&Arc<SharedAudioPlayback>>,
    ) {
        let Some(shared) = shared else {
            self.opacity = 0.0;
            return;
        };
        if input_mode != InputMode::Audio {
            self.opacity = 0.0;
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
        self.fade_target = if hovering { 1.0 } else { 0.0 };
        let dt = ctx.input(|i| i.predicted_dt);
        let fade_speed = 1.0 / 0.2; // 200ms fade
        if self.opacity < self.fade_target {
            self.opacity = (self.opacity + dt * fade_speed).min(self.fade_target);
        } else {
            self.opacity = (self.opacity - dt * fade_speed).max(self.fade_target);
        }

        if (self.opacity - self.fade_target).abs() > 0.01 {
            ctx.request_repaint();
        }

        if self.opacity < 0.01 {
            return;
        }

        let alpha = (self.opacity * 255.0) as u8;

        egui::Area::new(egui::Id::new("media_overlay"))
            .fixed_pos(overlay_rect.left_top())
            .show(ctx, |ui| {
                let frame = egui::Frame::new()
                    .fill(egui::Color32::from_black_alpha(
                        (180.0 * self.opacity) as u8,
                    ))
                    .corner_radius(8.0)
                    .inner_margin(8.0);

                frame.show(ui, |ui| {
                    ui.set_min_width(overlay_width - 16.0);
                    ui.set_max_width(overlay_width - 16.0);

                    ui.visuals_mut().override_text_color =
                        Some(egui::Color32::from_white_alpha(alpha));

                    ui.horizontal(|ui| {
                        use std::sync::atomic::Ordering;

                        // Play/Pause
                        let playing = shared.playing.load(Ordering::Relaxed);
                        let label = if playing { "\u{23F8}" } else { "\u{25B6}" };
                        if ui.button(label).clicked() {
                            shared.playing.store(!playing, Ordering::Relaxed);
                        }

                        // Seek bar
                        let duration = shared.duration_secs();
                        let mut pos = shared.position_secs();
                        let slider = egui::Slider::new(&mut pos, 0.0..=duration)
                            .show_value(false)
                            .trailing_fill(true);
                        if ui.add(slider).changed() {
                            shared.seek_to_secs(pos);
                        }

                        // Time label
                        let pos_str = super::scope_panel::format_time(pos);
                        let dur_str = super::scope_panel::format_time(duration);
                        ui.label(format!("{pos_str} / {dur_str}"));
                    });
                });
            });
    }
}
