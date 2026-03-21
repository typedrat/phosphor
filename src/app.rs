use std::sync::Arc;
use std::time::{Duration, Instant};

use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow};
use winit::window::{Window, WindowId};

use crate::audio_output::{AudioOutput, SharedAudioPlayback};
use crate::beam::{BeamSample, SampleConsumer};
use crate::controls_window::ControlsWindow;
use crate::gpu::GpuState;
use crate::recording::{RecordingConfig, RecordingState};
use crate::simulation::SimCommand;
use crate::simulation_stats::SimStats;
use crate::types::InputMode;
use crate::ui::recording_panel::RecordingProgress;
use crate::ui::{SimFrameInfo, UiState};

#[derive(Default, PartialEq)]
enum WindowMode {
    Combined,
    #[default]
    Detached,
}

enum GlobalAction {
    Quit,
    ToggleDetach,
    ToggleFullscreen,
}

fn check_global_shortcut(
    event: &WindowEvent,
    modifiers: &winit::keyboard::ModifiersState,
) -> Option<GlobalAction> {
    let WindowEvent::KeyboardInput {
        event:
            winit::event::KeyEvent {
                physical_key: winit::keyboard::PhysicalKey::Code(key_code),
                state: winit::event::ElementState::Pressed,
                ..
            },
        ..
    } = event
    else {
        return None;
    };

    let has_modifier = modifiers.control_key() || modifiers.super_key();
    if !has_modifier {
        return None;
    }

    match key_code {
        winit::keyboard::KeyCode::KeyQ => Some(GlobalAction::Quit),
        winit::keyboard::KeyCode::KeyD => Some(GlobalAction::ToggleDetach),
        winit::keyboard::KeyCode::KeyF => Some(GlobalAction::ToggleFullscreen),
        _ => None,
    }
}

/// Fallback frame interval when the monitor refresh rate can't be queried.
const DEFAULT_FRAME_INTERVAL: Duration = Duration::from_micros(16_667); // 60 Hz

pub struct App {
    // Drop order matters: GPU resources (surfaces) must be dropped before the
    // windows they reference, so `gpu` and `controls` are declared before `window`.
    gpu: Option<GpuState>,
    controls: Option<ControlsWindow>,
    ui: Option<UiState>,
    mode: WindowMode,
    window: Option<Arc<Window>>,
    frame_interval: Duration,
    next_frame: Instant,
    // Simulation thread
    sim_consumer: Option<SampleConsumer>,
    sim_commands: Option<crossbeam_channel::Sender<SimCommand>>,
    sim_handle: Option<std::thread::JoinHandle<()>>,
    sim_stats: Option<Arc<SimStats>>,
    sample_rate: f32,
    audio_output: Option<AudioOutput>,
    modifiers: winit::keyboard::ModifiersState,
    // Recording
    recording: Option<RecordingState>,
    batch_request_tx: Option<crossbeam_channel::Sender<()>>,
    batch_response_rx: Option<crossbeam_channel::Receiver<Vec<BeamSample>>>,
    recording_cancel_requested: bool,
}

impl Default for App {
    fn default() -> Self {
        Self {
            gpu: None,
            controls: None,
            ui: None,
            mode: WindowMode::default(),
            window: None,
            frame_interval: DEFAULT_FRAME_INTERVAL,
            next_frame: Instant::now(),
            sim_consumer: None,
            sim_commands: None,
            sim_handle: None,
            sim_stats: None,
            sample_rate: 44100.0,
            audio_output: None,
            modifiers: winit::keyboard::ModifiersState::empty(),
            recording: None,
            batch_request_tx: None,
            batch_response_rx: None,
            recording_cancel_requested: false,
        }
    }
}

impl App {
    fn spawn_audio_decode(&mut self) {
        let Some(ui) = &mut self.ui else { return };
        let Some(path) = ui.audio_ui.pending_file.take() else {
            return;
        };

        ui.audio_ui.file_path = Some(path.clone());
        ui.audio_ui.load_error = None;

        let (tx, rx) = crossbeam_channel::bounded(1);
        ui.audio_ui.decode_receiver = Some(rx);

        std::thread::Builder::new()
            .name("audio-decode".into())
            .spawn(move || {
                let result = crate::beam::audio::decode_audio_file(&path);
                let _ = tx.send(result);
            })
            .expect("failed to spawn audio decode thread");
    }

    fn poll_audio_decode(&mut self) {
        let Some(ui) = &mut self.ui else { return };
        let Some(rx) = &ui.audio_ui.decode_receiver else {
            return;
        };

        match rx.try_recv() {
            Ok(Ok(decoded)) => {
                let audio_rate = decoded.sample_rate as f32;
                let shared = Arc::new(SharedAudioPlayback::new(decoded));
                shared
                    .playing
                    .store(true, std::sync::atomic::Ordering::Relaxed);
                shared
                    .looping
                    .store(true, std::sync::atomic::Ordering::Relaxed);

                // Create audio output (fallible — degrade to visual-only)
                match AudioOutput::new(Arc::clone(&shared)) {
                    Ok(output) => {
                        self.audio_output = Some(output);
                    }
                    Err(e) => {
                        tracing::warn!("No audio output: {e:#}");
                        ui.audio_ui.load_error = Some(format!("Audio output unavailable: {e:#}"));
                    }
                }

                // Update sample rate and resize ring buffer to match audio
                if audio_rate != self.sample_rate {
                    self.sample_rate = audio_rate;
                    let capacity = (self.sample_rate as usize * 3 / 2).next_power_of_two();
                    let (producer, consumer) = crate::beam::sample_channel(capacity);
                    self.sim_consumer = Some(consumer);
                    if let Some(tx) = &self.sim_commands {
                        let _ = tx.send(SimCommand::SetSampleRate {
                            rate: self.sample_rate,
                            producer,
                        });
                    }
                }

                // Send shared state to sim thread
                if let Some(tx) = &self.sim_commands {
                    let _ = tx.send(SimCommand::SetAudioShared(Some(Arc::clone(&shared))));
                }

                ui.audio_ui.shared = Some(shared);
                ui.audio_ui.decode_receiver = None;
            }
            Ok(Err(e)) => {
                ui.audio_ui.load_error = Some(format!("{e:#}"));
                ui.audio_ui.decode_receiver = None;
            }
            Err(crossbeam_channel::TryRecvError::Empty) => {
                // Still decoding
            }
            Err(crossbeam_channel::TryRecvError::Disconnected) => {
                ui.audio_ui.load_error = Some("Decode thread crashed".to_string());
                ui.audio_ui.decode_receiver = None;
            }
        }
    }

    fn start_recording(&mut self) {
        // Check ffmpeg availability
        if let Err(e) = crate::recording::ffmpeg::check_ffmpeg() {
            tracing::error!("Cannot record: {e}");
            return;
        }

        let gpu = match &mut self.gpu {
            Some(g) => g,
            None => return,
        };
        let ui = match &mut self.ui {
            Some(u) => u,
            None => return,
        };
        let sim_tx = match &self.sim_commands {
            Some(tx) => tx,
            None => return,
        };

        let (width, height) = ui
            .recording
            .effective_resolution(gpu.device.limits().max_texture_dimension_2d);
        let fps = ui.recording.effective_fps();

        // Calculate total frames
        let total_frames = if ui.input_mode == InputMode::Audio {
            if let Some(shared) = &ui.audio_ui.shared {
                let total_audio_frames = shared.samples.len() / shared.channels as usize;
                let duration_secs = total_audio_frames as f64 / shared.sample_rate as f64;
                (duration_secs * fps as f64).ceil() as u64
            } else {
                tracing::error!("No audio loaded for Audio mode recording");
                return;
            }
        } else {
            (ui.recording.duration_secs as f64 * fps as f64).ceil() as u64
        };

        let samples_per_frame = (self.sample_rate / fps as f32).round() as usize;

        // Create batch channels
        let (req_tx, req_rx) = crossbeam_channel::bounded(1);
        let (resp_tx, resp_rx) = crossbeam_channel::unbounded();

        // Send batch mode command to sim thread
        let _ = sim_tx.send(SimCommand::StartBatchMode {
            samples_per_frame,
            frame_request_rx: req_rx,
            frame_response_tx: resp_tx,
        });

        // Rewind audio for recording
        if ui.input_mode == InputMode::Audio {
            let _ = sim_tx.send(SimCommand::RewindRecordingAudio);
        }

        // Prepare GPU for offscreen rendering
        let resolution = crate::types::Resolution::new(width, height);
        gpu.prepare_recording(resolution);

        // Set viewport offset to 0 for recording (no sidebar)
        gpu.composite_params.viewport_offset = [0.0, 0.0];
        gpu.composite_params.viewport_size = [width as f32, height as f32];

        let output_path = match &ui.recording.output_path {
            Some(p) => p.clone(),
            None => {
                tracing::error!("No output path set");
                let _ = sim_tx.send(SimCommand::StopBatchMode);
                return;
            }
        };

        let custom_args = if ui.recording.custom_ffmpeg_args.trim().is_empty() {
            None
        } else {
            Some(ui.recording.custom_ffmpeg_args.clone())
        };

        let audio_path = if ui.input_mode == InputMode::Audio {
            ui.audio_ui.file_path.clone()
        } else {
            None
        };

        let config = RecordingConfig {
            width,
            height,
            fps,
            output_path,
            audio_path,
            preset: ui.recording.encoding_preset,
            custom_args,
            pre_roll_frames: fps, // 1 second of pre-roll
            total_frames,
        };

        match RecordingState::start(&gpu.device, config, self.sample_rate as u32) {
            Ok(state) => {
                tracing::info!(
                    "Recording started: {}x{} @ {} fps, {} frames",
                    width,
                    height,
                    fps,
                    total_frames
                );
                self.recording = Some(state);
                self.batch_request_tx = Some(req_tx);
                self.batch_response_rx = Some(resp_rx);
                self.recording_cancel_requested = false;
            }
            Err(e) => {
                tracing::error!("Failed to start recording: {e}");
                let _ = sim_tx.send(SimCommand::StopBatchMode);
                gpu.end_recording();
                ui.recording.recording_progress = None;
            }
        }
    }

    fn stop_recording(&mut self) {
        if let Some(state) = self.recording.take() {
            let device = self.gpu.as_ref().map(|g| &g.device);
            if let Some(device) = device {
                if let Err(e) = state.finish(device) {
                    tracing::error!("Error finishing recording: {e}");
                }
            } else {
                tracing::error!("No GPU device available to flush final recording frame");
            }
        }

        if let Some(tx) = &self.sim_commands {
            let _ = tx.send(SimCommand::StopBatchMode);
        }

        self.batch_request_tx = None;
        self.batch_response_rx = None;
        self.recording_cancel_requested = false;

        if let Some(gpu) = &mut self.gpu {
            gpu.end_recording();
        }

        if let Some(ui) = &mut self.ui {
            ui.recording.recording_progress = None;
        }
    }

    fn toggle_detach(&mut self, event_loop: &ActiveEventLoop) {
        match self.mode {
            WindowMode::Combined => {
                let Some(gpu) = &self.gpu else { return };
                let Some(ui) = &self.ui else { return };
                if let Some(controls) = ControlsWindow::new(event_loop, gpu, ui.ctx.clone()) {
                    self.controls = Some(controls);
                    self.mode = WindowMode::Detached;
                    if let Some(ui) = &mut self.ui {
                        ui.panel_visible = false;
                    }
                    tracing::info!("Detached controls to separate window");
                }
            }
            WindowMode::Detached => {
                self.controls = None;
                self.mode = WindowMode::Combined;
                if let Some(ui) = &mut self.ui {
                    ui.panel_visible = true;
                }
                tracing::info!("Combined controls back into main window");
            }
        }
    }

    fn handle_viewport_event(&mut self, event_loop: &ActiveEventLoop, event: WindowEvent) {
        // Always forward events to the media overlay (works in both modes)
        if let Some(ui) = &mut self.ui
            && let Some(window) = &self.window
        {
            ui.media_overlay.on_event(window, &event);
        }

        // Only pass events to the shared egui context in Combined mode
        // (in Detached mode, the panel renders on the controls window)
        if self.mode == WindowMode::Combined
            && let Some(ui) = &mut self.ui
            && let Some(window) = &self.window
        {
            let response = ui.on_event(window, &event);
            if response.consumed {
                return;
            }
        }

        match event {
            WindowEvent::CloseRequested => {
                self.audio_output = None;
                if let Some(tx) = self.sim_commands.take() {
                    let _ = tx.send(SimCommand::Shutdown);
                }
                if let Some(handle) = self.sim_handle.take() {
                    let _ = handle.join();
                }
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                let scale = self
                    .ui
                    .as_ref()
                    .map_or(1.0, |ui| ui.engineer.accum_resolution_scale);
                if let Some(gpu) = &mut self.gpu {
                    gpu.resize(size.width, size.height, scale);
                }
            }
            WindowEvent::RedrawRequested => {
                // Check recording start/stop signals before borrowing ui
                let want_start = self
                    .ui
                    .as_ref()
                    .is_some_and(|ui| ui.recording.start_requested);
                let want_cancel = self
                    .ui
                    .as_ref()
                    .is_some_and(|ui| ui.recording.cancel_requested);

                if want_start {
                    if let Some(ui) = &mut self.ui {
                        ui.recording.start_requested = false;
                    }
                    self.start_recording();
                }
                if want_cancel || self.recording_cancel_requested {
                    if let Some(ui) = &mut self.ui {
                        ui.recording.cancel_requested = false;
                    }
                    self.stop_recording();
                }

                let is_recording = self.recording.is_some();

                // Audio file loading (background decode)
                self.spawn_audio_decode();
                self.poll_audio_decode();

                let Some(window) = &self.window else { return };
                let Some(gpu) = &mut self.gpu else { return };
                let Some(ui) = &mut self.ui else { return };

                // Phosphor change: rebuild decay/emission/spectral params + buffer
                if !is_recording && ui.phosphor_changed() {
                    gpu.switch_phosphor(ui.selected_phosphor());
                }

                // Apply UI state to GPU parameters (skip during recording)
                crate::frame::sync_gpu_params(gpu, ui, is_recording);

                // Feed accumulation buffer size to UI for display
                ui.accum_size = Some(gpu.accum.resolution);

                // Sample acquisition: batch mode when recording, ring buffer otherwise
                let (samples, sim_dt) = if is_recording {
                    if let (Some(req_tx), Some(resp_rx)) =
                        (&self.batch_request_tx, &self.batch_response_rx)
                    {
                        let _ = req_tx.send(());
                        match resp_rx.recv() {
                            Ok(batch) => {
                                let dt = self.recording.as_ref().map_or(0.0, |r| r.dt);
                                (batch, dt)
                            }
                            Err(_) => {
                                tracing::error!("Batch response channel disconnected");
                                self.recording_cancel_requested = true;
                                (vec![], 0.0)
                            }
                        }
                    } else {
                        (vec![], 0.0)
                    }
                } else {
                    // Drain samples from simulation thread's ring buffer.
                    // Cap at 8x frame interval — arc-length resampling can expand
                    // audio samples significantly, so 2x was too tight.
                    let max_dt = self.frame_interval.as_secs_f32() * 8.0;
                    let max_samples = (self.sample_rate * max_dt) as usize;
                    let drained = self
                        .sim_consumer
                        .as_mut()
                        .map(|c| c.drain_up_to(max_samples))
                        .unwrap_or_default();
                    let dt = if drained.is_empty() {
                        0.0
                    } else {
                        drained.len() as f32 / self.sample_rate
                    };
                    (drained, dt)
                };

                // Build per-frame simulation info for the engineer panel
                let sim_frame_info = SimFrameInfo {
                    samples_this_frame: samples.len(),
                    sim_dt,
                    buffer_pending: self.sim_consumer.as_ref().map_or(0, |c| c.pending()),
                };

                // Run shared egui frame only in Combined mode (panel + sidebar)
                let egui_output = if self.mode == WindowMode::Combined {
                    let timings = gpu.profiler.as_ref().map(|p| &p.history);
                    Some(ui.run(
                        window,
                        timings,
                        self.sim_stats.as_ref(),
                        Some(&sim_frame_info),
                    ))
                } else {
                    None
                };

                // Run media overlay (separate egui context, works in both modes)
                let sc = gpu.surface_config.as_ref().unwrap();
                let viewport_rect = egui::Rect::from_min_size(
                    egui::pos2(ui.panel_width, 0.0),
                    egui::vec2(sc.width as f32 - ui.panel_width, sc.height as f32),
                );
                let overlay_output = ui.media_overlay.run(
                    window,
                    viewport_rect,
                    ui.input_mode,
                    ui.audio_ui.shared.as_ref(),
                );

                // Forward UI state changes to the simulation thread (skip during recording)
                if !is_recording {
                    let sidebar_width = if self.mode == WindowMode::Combined {
                        ui.panel_width
                    } else {
                        0.0
                    };
                    gpu.composite_params.viewport_offset = [sidebar_width, 0.0];
                    let sc = gpu.surface_config.as_ref().unwrap();
                    gpu.composite_params.viewport_size =
                        [sc.width as f32 - sidebar_width, sc.height as f32];

                    if let Some(tx) = &self.sim_commands {
                        crate::frame::dispatch_sim_commands(
                            tx,
                            ui,
                            gpu,
                            sidebar_width,
                            &mut self.sample_rate,
                            &mut self.sim_consumer,
                        );
                    }
                }

                // Build overlay render args (need mutable ref to overlay renderer)
                let overlay_render = overlay_output
                    .as_ref()
                    .and_then(|output| ui.media_overlay.renderer.as_mut().map(|r| (r, output)));

                // Get offscreen view for recording
                let offscreen_view = self.recording.as_ref().map(|r| &r.offscreen_view);

                match gpu.render(
                    &samples,
                    sim_dt,
                    egui_output.as_ref(),
                    overlay_render,
                    offscreen_view,
                ) {
                    Ok(()) => {}
                    Err(wgpu::SurfaceError::Lost) => {
                        let sc = gpu.surface_config.as_ref().unwrap();
                        let (w, h) = (sc.width, sc.height);
                        gpu.resize(w, h, ui.engineer.accum_resolution_scale);
                    }
                    Err(wgpu::SurfaceError::OutOfMemory) => {
                        tracing::error!("GPU out of memory");
                        event_loop.exit();
                    }
                    Err(e) => {
                        tracing::warn!("Surface error: {e:?}");
                    }
                }

                // Recording: readback + pipe + advance
                if let Some(recording) = &mut self.recording {
                    // Encode copy from offscreen texture to staging buffer
                    let mut encoder =
                        gpu.device
                            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                                label: Some("recording_readback"),
                            });
                    recording
                        .readback
                        .copy_from_texture(&mut encoder, &recording.offscreen_texture);
                    gpu.queue.submit(std::iter::once(encoder.finish()));

                    if !recording.is_pre_roll() {
                        // Read back the PREVIOUS frame (double-buffered, one frame behind)
                        if let Some(data) = recording.readback.read_pending(&gpu.device)
                            && let Err(e) = recording.pipe_writer.send(data) {
                                tracing::error!("Failed to send frame to pipe writer: {e}");
                                self.recording_cancel_requested = true;
                            }
                    } else {
                        // Still need to let the GPU finish the copy before next frame
                        let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
                    }

                    recording.readback.advance();

                    let preroll_just_ended = recording.advance_frame();
                    if preroll_just_ended {
                        // Rewind audio to start so recorded video begins at t=0
                        if let Some(ref tx) = self.sim_commands {
                            let _ = tx.send(SimCommand::RewindRecordingAudio);
                        }
                        tracing::info!("Pre-roll complete, audio rewound to start");
                    }

                    // Update UI progress
                    ui.recording.recording_progress = Some(RecordingProgress {
                        current_frame: recording.current_frame,
                        total_frames: recording.total_frames,
                        elapsed: recording.elapsed(),
                        pre_rolling: recording.is_pre_roll(),
                    });

                    // Check completion
                    if recording.is_complete() {
                        tracing::info!("Recording complete");
                        // Need to stop recording — set flag for next frame
                        // (can't call self.stop_recording() while borrowing self.recording)
                        self.recording_cancel_requested = true;
                    }
                }
            }
            _ => {}
        }
    }

    fn handle_controls_event(&mut self, event_loop: &ActiveEventLoop, event: WindowEvent) {
        // Pass events to controls egui_winit
        if let Some(controls) = &mut self.controls {
            let _response = controls
                .egui_winit
                .on_window_event(&controls.window, &event);
        }

        match event {
            WindowEvent::CloseRequested => {
                // Recombine: drop controls, go back to Combined mode
                self.controls = None;
                self.mode = WindowMode::Combined;
                if let Some(ui) = &mut self.ui {
                    ui.panel_visible = true;
                }
                tracing::info!("Controls window closed, recombined into main window");
            }
            WindowEvent::Resized(size) => {
                if let Some(controls) = &mut self.controls
                    && size.width > 0
                    && size.height > 0
                {
                    controls.surface_config.width = size.width;
                    controls.surface_config.height = size.height;
                    if let Some(gpu) = &self.gpu {
                        controls
                            .surface
                            .configure(&gpu.device, &controls.surface_config);
                    }
                }
            }
            WindowEvent::RedrawRequested => {
                let (controls, gpu, ui) = match (&mut self.controls, &self.gpu, &mut self.ui) {
                    (Some(c), Some(g), Some(u)) => (c, g, u),
                    _ => return,
                };
                match controls.render(gpu, ui, self.sim_stats.as_ref()) {
                    Ok(()) => {}
                    Err(wgpu::SurfaceError::Lost) => {
                        controls
                            .surface
                            .configure(&gpu.device, &controls.surface_config);
                    }
                    Err(wgpu::SurfaceError::OutOfMemory) => {
                        tracing::error!("GPU out of memory (controls window)");
                        event_loop.exit();
                    }
                    Err(e) => {
                        tracing::warn!("Controls surface error: {e:?}");
                    }
                }
            }
            _ => {}
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let attrs = Window::default_attributes().with_title("Phosphor");

        let window: Arc<Window> = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                tracing::error!("Failed to create window: {e}");
                event_loop.exit();
                return;
            }
        };

        // Query the monitor's native refresh rate for frame pacing.
        // Fifo present mode alone isn't reliable on all Linux compositors,
        // so we also pace via ControlFlow::WaitUntil in about_to_wait.
        if let Some(monitor) = window.current_monitor()
            && let Some(millihertz) = monitor.refresh_rate_millihertz()
        {
            let micros = 1_000_000_000 / millihertz as u64;
            self.frame_interval = Duration::from_micros(micros);
            tracing::info!(
                "Monitor refresh rate: {:.1} Hz (frame interval: {:.2} ms)",
                millihertz as f64 / 1000.0,
                micros as f64 / 1000.0,
            );
        }

        let mut gpu = GpuState::new(window.clone());
        let mut ui = UiState::new(&window);
        ui.media_overlay.init(
            &window,
            &gpu.device,
            gpu.surface_config.as_ref().unwrap().format,
        );
        gpu.switch_phosphor(ui.selected_phosphor());

        // Spawn simulation thread
        let buffer_capacity = 65536;
        let (producer, consumer) = crate::beam::sample_channel(buffer_capacity);
        let stats = SimStats::new(buffer_capacity as u32);
        let (handle, cmd_tx) = crate::simulation::spawn_simulation(producer, stats.clone());

        // Send initial viewport dimensions
        let size = window.inner_size();
        let _ = cmd_tx.send(SimCommand::SetViewport {
            width: size.width as f32,
            height: size.height as f32,
            x_offset: 0.0,
        });

        // If starting in detached mode, create the controls window immediately
        if self.mode == WindowMode::Detached
            && let Some(controls) = ControlsWindow::new(event_loop, &gpu, ui.ctx.clone())
        {
            self.controls = Some(controls);
        }

        self.sim_consumer = Some(consumer);
        self.sim_commands = Some(cmd_tx);
        self.sim_handle = Some(handle);
        self.sim_stats = Some(stats);
        self.window = Some(window);
        self.gpu = Some(gpu);
        self.ui = Some(ui);
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        // Track modifier keys from any window
        if let WindowEvent::ModifiersChanged(mods) = &event {
            self.modifiers = mods.state();
        }

        if let Some(action) = check_global_shortcut(&event, &self.modifiers) {
            match action {
                GlobalAction::Quit => event_loop.exit(),
                GlobalAction::ToggleDetach => self.toggle_detach(event_loop),
                GlobalAction::ToggleFullscreen => {
                    if let Some(window) = &self.window {
                        if window.fullscreen().is_some() {
                            window.set_fullscreen(None);
                        } else {
                            window
                                .set_fullscreen(Some(winit::window::Fullscreen::Borderless(None)));
                        }
                    }
                }
            }
            return;
        }

        // Route by window ID
        let is_viewport = self.window.as_ref().is_some_and(|w| w.id() == window_id);
        let is_controls = self
            .controls
            .as_ref()
            .is_some_and(|c| c.window.id() == window_id);

        if is_controls {
            self.handle_controls_event(event_loop, event);
        } else if is_viewport {
            self.handle_viewport_event(event_loop, event);
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        if let Some(controls) = &self.controls {
            controls.window.request_redraw();
        }

        if self.recording.is_some() {
            // Recording mode: render as fast as possible
            event_loop.set_control_flow(ControlFlow::Poll);
        } else {
            // Pace frames to the monitor's native refresh rate. Fifo present
            // mode should do this via swapchain blocking, but doesn't reliably
            // engage on all Linux Vulkan compositors.
            self.next_frame += self.frame_interval;
            // If we fell behind (e.g. long frame), reset to avoid a burst of catch-up frames
            let now = Instant::now();
            if self.next_frame < now {
                self.next_frame = now + self.frame_interval;
            }
            event_loop.set_control_flow(ControlFlow::WaitUntil(self.next_frame));
        }
    }
}
