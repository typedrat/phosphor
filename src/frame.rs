use crate::beam::SampleConsumer;
use crate::gpu::GpuState;
use crate::simulation::SimCommand;
use crate::types::Resolution;
use crate::ui::UiState;

/// Apply UI state to GPU pipeline parameters. Called once per frame.
pub fn sync_gpu_params(gpu: &mut GpuState, ui: &UiState) {
    let eng = &ui.engineer;
    let scale = eng.accum_resolution_scale;

    // Beam -- scope focus overrides core sigma, engineer controls the rest.
    // Sigma values are in accum-buffer pixels, so scale by buffer scale
    // to keep the visual beam width constant regardless of resolution.
    gpu.beam_params.sigma_core = ui.focus * scale;
    gpu.beam_params.sigma_halo = eng.sigma_halo * scale;
    gpu.beam_params.halo_fraction = eng.halo_fraction;

    // Faceplate scatter — sigma is in half-res texels, same scaling
    gpu.faceplate_scatter_params.threshold = eng.scatter_threshold;
    gpu.faceplate_scatter_params.sigma = eng.scatter_sigma * scale;
    gpu.faceplate_scatter_params.intensity = eng.scatter_intensity;

    // Composite / display
    gpu.composite_params.exposure = ui.intensity;
    gpu.composite_params.set_mode(eng.tonemap_mode);
    gpu.composite_params.faceplate_scatter_intensity = eng.scatter_intensity;
    gpu.composite_params.glass_tint = eng.glass_tint;
    gpu.composite_params.curvature = eng.curvature;
    gpu.composite_params.edge_falloff = eng.edge_falloff;

    // Accumulation buffer resize if resolution scale changed
    let target = Resolution::new(
        ((gpu.surface_config.width as f32) * scale).round().max(1.0) as u32,
        ((gpu.surface_config.height as f32) * scale)
            .round()
            .max(1.0) as u32,
    );
    if target != gpu.accum.resolution {
        gpu.resize_buffers(target);
    }
}

/// Send current UI state to the simulation thread. Called once per frame.
pub fn dispatch_sim_commands(
    tx: &crossbeam_channel::Sender<SimCommand>,
    ui: &mut UiState,
    gpu: &GpuState,
    sidebar_width: f32,
    sample_rate: &mut f32,
    sim_consumer: &mut Option<SampleConsumer>,
) {
    let _ = tx.send(SimCommand::SetInputMode(ui.input_mode));
    let _ = tx.send(SimCommand::SetOscilloscopeParams(ui.oscilloscope.clone()));
    let _ = tx.send(SimCommand::SetFocus(ui.focus));
    let _ = tx.send(SimCommand::SetViewport {
        width: gpu.surface_config.width as f32 - sidebar_width,
        height: gpu.surface_config.height as f32,
        x_offset: sidebar_width,
    });

    // Audio controls
    let _ = tx.send(SimCommand::SetAudioPlaying(ui.audio_ui.playing));
    let _ = tx.send(SimCommand::SetAudioLooping(ui.audio_ui.looping));
    let _ = tx.send(SimCommand::SetAudioSpeed(ui.audio_ui.speed));
    if let Some(path) = ui.audio_ui.pending_file.take() {
        ui.audio_ui.file_path = Some(path.clone());
        ui.audio_ui.has_file = true;
        let _ = tx.send(SimCommand::LoadAudioFile(path));
    }

    // Vector controls
    if let Some(path) = ui.vector_ui.pending_file.take() {
        ui.vector_ui.file_path = Some(path.clone());
        let _ = tx.send(SimCommand::LoadVectorFile(path));
    }

    // Sample rate change — recreate the ring buffer
    if ui.oscilloscope.sample_rate != *sample_rate {
        *sample_rate = ui.oscilloscope.sample_rate;
        let capacity = (*sample_rate as usize * 3 / 2).next_power_of_two();
        let (producer, consumer) = crate::beam::sample_channel(capacity);
        *sim_consumer = Some(consumer);
        let _ = tx.send(SimCommand::SetSampleRate {
            rate: *sample_rate,
            producer,
        });
    }
}
