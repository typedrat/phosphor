use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::Receiver;

use crate::audio_output::SharedAudioPlayback;
use crate::beam::oscilloscope::{ChannelConfig, OscilloscopeSource};
use crate::beam::vector::VectorSegment;
use crate::beam::{BeamSample, BeamSource, BeamState, SampleProducer};
use crate::simulation_stats::SimStats;
use crate::types::{ExternalState, InputMode, OscilloscopeState};

struct BatchModeState {
    samples_per_frame: usize,
    frame_request_rx: crossbeam_channel::Receiver<()>,
    frame_response_tx: crossbeam_channel::Sender<Vec<BeamSample>>,
}

/// Calibration constant for beam energy deposition. The beam_write shader
/// computes `energy = intensity * profile * dt`, where dt is the per-sample
/// dwell time (~1/44100 s). Without scaling, the deposited energy is on the
/// order of 1e-5, which is invisible after spectral integration and
/// tonemapping. This constant represents the beam current / power scale
/// that makes the phosphor visibly glow at the default settings.
const BEAM_ENERGY_SCALE: f32 = 5000.0;

#[derive(Default)]
pub struct AudioState {
    pub shared: Option<Arc<SharedAudioPlayback>>,
    pub last_audio_pos: usize,
    pub recording_audio_pos: usize,
}

pub struct VectorState {
    pub file_path: Option<PathBuf>,
    pub segments: Vec<VectorSegment>,
    pub beam_speed: f32,
    pub settling_time: f32,
    pub looping: bool,
    pub load_error: Option<String>,
}

impl Default for VectorState {
    fn default() -> Self {
        Self {
            file_path: None,
            segments: Vec::new(),
            beam_speed: 1.0,
            settling_time: 0.001,
            looping: true,
            load_error: None,
        }
    }
}

pub struct InputState {
    pub mode: InputMode,
    pub oscilloscope: OscilloscopeState,
    pub audio: AudioState,
    pub vector: VectorState,
    pub external: ExternalState,
    osc_source: OscilloscopeSource,
}

impl Default for InputState {
    fn default() -> Self {
        let osc = OscilloscopeState::default();
        let osc_source = OscilloscopeSource::new(
            ChannelConfig {
                waveform: osc.x_waveform,
                frequency: osc.x_frequency,
                amplitude: osc.x_amplitude,
                phase: osc.x_phase,
                dc_offset: osc.x_dc_offset,
            },
            ChannelConfig {
                waveform: osc.y_waveform,
                frequency: osc.y_frequency,
                amplitude: osc.y_amplitude,
                phase: osc.y_phase,
                dc_offset: osc.y_dc_offset,
            },
            osc.sample_rate,
        );

        Self {
            mode: InputMode::default(),
            oscilloscope: osc,
            audio: AudioState::default(),
            vector: VectorState::default(),
            external: ExternalState::default(),
            osc_source,
        }
    }
}

impl InputState {
    /// Generate a fixed number of samples at the given sample rate.
    /// Unlike `generate_samples`, this does NOT measure wall-clock time —
    /// dt is always `1/sample_rate`, making output deterministic.
    pub fn generate_samples_fixed(
        &mut self,
        focus: f32,
        aspect: f32,
        viewport_width: f32,
        sample_rate: f32,
        count: usize,
    ) -> Vec<BeamSample> {
        let spot_radius = focus / viewport_width.max(1.0);
        let beam = BeamState { spot_radius };

        let mut samples = match self.mode {
            InputMode::Oscilloscope => {
                self.sync_oscilloscope_params();
                self.osc_source.sample_rate = sample_rate;
                if count == 0 {
                    return Vec::new();
                }
                self.osc_source.generate(count, &beam)
            }
            InputMode::Audio => {
                let audio = &mut self.audio;
                let Some(shared) = &audio.shared else {
                    return Vec::new();
                };
                if !shared.playing.load(Ordering::Relaxed) {
                    return Vec::new();
                }

                let current_pos = shared.position.load(Ordering::Relaxed);
                let last_pos = audio.last_audio_pos;
                let channels = shared.channels as usize;
                let dt = 1.0 / shared.sample_rate as f32;

                // Detect discontinuity (seek or loop): position jumped backwards,
                // or jumped forward by more than ~100ms of audio.
                let max_reasonable_delta = (shared.sample_rate as f64 * 0.1) as usize;
                let is_discontinuity = current_pos < last_pos
                    || current_pos.wrapping_sub(last_pos) > max_reasonable_delta;

                if is_discontinuity {
                    audio.last_audio_pos = current_pos;
                    return Vec::new();
                }

                // Generate beam samples for [last_pos, current_pos)
                let delta = current_pos - last_pos;
                let samples_data = &shared.samples;
                let mut result = Vec::with_capacity(delta);
                for frame in last_pos..current_pos {
                    let idx = frame * channels;
                    if idx + 1 >= samples_data.len() {
                        break;
                    }
                    let l = samples_data[idx];
                    let r = samples_data[idx + 1];
                    result.push(BeamSample {
                        x: (l + 1.0) / 2.0,
                        y: (r + 1.0) / 2.0,
                        intensity: 1.0,
                        dt,
                    });
                }

                audio.last_audio_pos = current_pos;
                result
            }
            InputMode::Vector => {
                if self.vector.segments.is_empty() {
                    return Vec::new();
                }
                let mut src = crate::beam::vector::VectorSource {
                    segments: self.vector.segments.clone(),
                    beam_speed: self.vector.beam_speed,
                    settling_time: self.vector.settling_time,
                };
                src.generate(0, &beam)
            }
            InputMode::External => Vec::new(),
        };

        // Aspect ratio correction
        if aspect > 1.0 {
            for s in &mut samples {
                s.x = 0.5 + (s.x - 0.5) / aspect;
            }
        } else if aspect < 1.0 {
            for s in &mut samples {
                s.y = 0.5 + (s.y - 0.5) * aspect;
            }
        }

        // Arc-length resample
        let mut samples = crate::beam::resample::arc_length_resample(&samples, spot_radius * 0.5);

        // Scale beam energy
        for s in &mut samples {
            s.intensity *= BEAM_ENERGY_SCALE;
        }

        samples
    }

    /// Generate a fixed number of audio samples for recording mode.
    /// Unlike `generate_samples_fixed` for Audio mode, this reads sequentially
    /// through the decoded audio buffer rather than following the cpal playback
    /// position (which doesn't advance during recording).
    pub fn generate_audio_samples_recording(
        &mut self,
        focus: f32,
        aspect: f32,
        viewport_width: f32,
        count: usize,
    ) -> Vec<BeamSample> {
        let spot_radius = focus / viewport_width.max(1.0);

        let audio = &mut self.audio;
        let Some(shared) = &audio.shared else {
            return Vec::new();
        };

        let channels = shared.channels as usize;
        let dt = 1.0 / shared.sample_rate as f32;
        let start_pos = audio.recording_audio_pos;
        let samples_data = &shared.samples;
        let total_frames = samples_data.len() / channels;
        let end_pos = (start_pos + count).min(total_frames);

        let mut result = Vec::with_capacity(end_pos - start_pos);
        for frame in start_pos..end_pos {
            let idx = frame * channels;
            if idx + 1 >= samples_data.len() {
                break;
            }
            let l = samples_data[idx];
            let r = samples_data[idx + 1];
            result.push(BeamSample {
                x: (l + 1.0) / 2.0,
                y: (r + 1.0) / 2.0,
                intensity: 1.0,
                dt,
            });
        }

        audio.recording_audio_pos = end_pos;

        // Aspect ratio correction
        if aspect > 1.0 {
            for s in &mut result {
                s.x = 0.5 + (s.x - 0.5) / aspect;
            }
        } else if aspect < 1.0 {
            for s in &mut result {
                s.y = 0.5 + (s.y - 0.5) * aspect;
            }
        }

        // Arc-length resample
        let mut result = crate::beam::resample::arc_length_resample(&result, spot_radius * 0.5);

        // Scale beam energy
        for s in &mut result {
            s.intensity *= BEAM_ENERGY_SCALE;
        }

        result
    }

    pub fn rewind_recording_audio(&mut self) {
        self.audio.recording_audio_pos = 0;
    }

    fn sync_oscilloscope_params(&mut self) {
        let osc = &self.oscilloscope;
        self.osc_source.x_channel.waveform = osc.x_waveform;
        self.osc_source.x_channel.frequency = osc.x_frequency;
        self.osc_source.x_channel.amplitude = osc.x_amplitude;
        self.osc_source.x_channel.phase = osc.x_phase;
        self.osc_source.x_channel.dc_offset = osc.x_dc_offset;
        self.osc_source.y_channel.waveform = osc.y_waveform;
        self.osc_source.y_channel.frequency = osc.y_frequency;
        self.osc_source.y_channel.amplitude = osc.y_amplitude;
        self.osc_source.y_channel.phase = osc.y_phase;
        self.osc_source.y_channel.dc_offset = osc.y_dc_offset;
        self.osc_source.sample_rate = osc.sample_rate;
    }

    pub fn load_vector_file(&mut self, path: PathBuf) {
        match std::fs::read_to_string(&path) {
            Ok(contents) => match serde_json::from_str::<Vec<VectorSegment>>(&contents) {
                Ok(segments) => {
                    self.vector.segments = segments;
                    self.vector.file_path = Some(path);
                    self.vector.load_error = None;
                }
                Err(e) => {
                    self.vector.load_error = Some(e.to_string());
                    self.vector.segments.clear();
                }
            },
            Err(e) => {
                self.vector.load_error = Some(e.to_string());
                self.vector.segments.clear();
            }
        }
    }
}

/// Target batch interval bounds.
const MIN_BATCH_INTERVAL: Duration = Duration::from_millis(1);
const MAX_BATCH_INTERVAL: Duration = Duration::from_millis(10);

/// Commands sent from the render/UI thread to the simulation thread.
pub enum SimCommand {
    SetInputMode(InputMode),
    SetOscilloscopeParams(OscilloscopeState),
    SetFocus(f32),
    /// Viewport dimensions and offset for aspect ratio correction.
    /// `x_offset` is the sidebar width in pixels (0 when hidden or detached).
    SetViewport {
        width: f32,
        height: f32,
        x_offset: f32,
    },
    SetAudioShared(Option<Arc<SharedAudioPlayback>>),
    LoadVectorFile(PathBuf),
    /// Sample rate change — carries the new producer from a resized channel.
    /// The render thread creates the new channel and swaps its consumer.
    SetSampleRate {
        rate: f32,
        producer: SampleProducer,
    },
    /// Enter batch-on-demand mode for recording. The sim thread will wait for
    /// requests on `frame_request_rx` and respond with exactly `samples_per_frame`
    /// samples via `frame_response_tx`.
    StartBatchMode {
        samples_per_frame: usize,
        frame_request_rx: crossbeam_channel::Receiver<()>,
        frame_response_tx: crossbeam_channel::Sender<Vec<BeamSample>>,
    },
    /// Exit batch mode and resume normal real-time sample generation.
    StopBatchMode,
    /// Reset the recording audio position to the beginning of the audio buffer.
    RewindRecordingAudio,
    Shutdown,
}

/// State tracked by the simulation thread, derived from SimCommands.
struct SimState {
    input: InputState,
    focus: f32,
    viewport_width: f32,
    viewport_height: f32,
    sample_rate: f32,
}

impl SimState {
    fn new() -> Self {
        let input = InputState::default();
        let sample_rate = input.oscilloscope.sample_rate;
        Self {
            input,
            focus: 1.5,
            viewport_width: 800.0,
            viewport_height: 600.0,
            sample_rate,
        }
    }

    fn aspect(&self) -> f32 {
        self.viewport_width / self.viewport_height.max(1.0)
    }

    fn apply_command(&mut self, cmd: SimCommand) {
        match cmd {
            SimCommand::SetInputMode(mode) => self.input.mode = mode,
            SimCommand::SetOscilloscopeParams(params) => {
                self.input.oscilloscope = params;
            }
            SimCommand::SetFocus(f) => self.focus = f,
            SimCommand::SetViewport { width, height, .. } => {
                self.viewport_width = width;
                self.viewport_height = height;
            }
            SimCommand::SetAudioShared(shared) => {
                self.input.audio.last_audio_pos = shared
                    .as_ref()
                    .map_or(0, |s| s.position.load(Ordering::Relaxed));
                self.input.audio.shared = shared;
            }
            SimCommand::LoadVectorFile(path) => self.input.load_vector_file(path),
            SimCommand::SetSampleRate { rate, .. } => self.sample_rate = rate,
            SimCommand::Shutdown => {}              // handled by caller
            SimCommand::StartBatchMode { .. } => {} // handled by run_simulation
            SimCommand::StopBatchMode => {}         // handled by run_simulation
            SimCommand::RewindRecordingAudio => {}  // handled by run_simulation
        }
    }
}

/// Run the simulation loop on the current thread. Blocks until Shutdown
/// is received or the command channel is disconnected.
pub fn run_simulation(
    mut producer: SampleProducer,
    commands: Receiver<SimCommand>,
    stats: Arc<SimStats>,
) {
    let _span = tracing::info_span!("sim").entered();
    let mut state = SimState::new();

    tracing::info!(sample_rate = state.sample_rate, "thread started");

    let mut batch_interval = MIN_BATCH_INTERVAL;
    let mut next_tick = Instant::now();

    // Throughput tracking: count samples over a 1-second window
    let mut samples_this_second: usize = 0;
    let mut generated_this_second: usize = 0;
    let mut second_timer = Instant::now();

    // Batch-on-demand mode state (used for recording)
    let mut batch_mode: Option<BatchModeState> = None;

    loop {
        // Process all pending commands
        while let Ok(cmd) = commands.try_recv() {
            if matches!(cmd, SimCommand::Shutdown) {
                tracing::info!("thread shutting down");
                return;
            }
            // SetSampleRate carries a new producer — swap it before
            // apply_command, which only updates the rate field.
            if let SimCommand::SetSampleRate {
                rate,
                producer: new_prod,
            } = cmd
            {
                producer = new_prod;
                state.sample_rate = rate;
                tracing::info!(sample_rate = rate, "sample rate changed");
                continue;
            }
            match cmd {
                SimCommand::StartBatchMode {
                    samples_per_frame,
                    frame_request_rx,
                    frame_response_tx,
                } => {
                    tracing::info!(samples_per_frame, "entering batch mode");
                    batch_mode = Some(BatchModeState {
                        samples_per_frame,
                        frame_request_rx,
                        frame_response_tx,
                    });
                    continue;
                }
                SimCommand::StopBatchMode => {
                    tracing::info!("exiting batch mode");
                    batch_mode = None;
                    next_tick = Instant::now();
                    continue;
                }
                SimCommand::RewindRecordingAudio => {
                    state.input.rewind_recording_audio();
                    continue;
                }
                cmd => {
                    state.apply_command(cmd);
                }
            }
        }

        // Batch-on-demand mode: wait for a frame request and respond with samples
        if let Some(ref bm) = batch_mode {
            // Block until the recorder requests a frame (or the channel closes)
            if bm.frame_request_rx.recv().is_err() {
                // Requester disconnected — exit batch mode
                tracing::info!("batch mode request channel closed, exiting batch mode");
                batch_mode = None;
                next_tick = Instant::now();
                continue;
            }

            // Re-borrow after recv (bm borrow ended)
            let (samples_per_frame, response_tx) = {
                let bm = batch_mode.as_ref().unwrap();
                (bm.samples_per_frame, bm.frame_response_tx.clone())
            };

            let samples = if state.input.mode == InputMode::Audio {
                state.input.generate_audio_samples_recording(
                    state.focus,
                    state.aspect(),
                    state.viewport_width,
                    samples_per_frame,
                )
            } else {
                state.input.generate_samples_fixed(
                    state.focus,
                    state.aspect(),
                    state.viewport_width,
                    state.sample_rate,
                    samples_per_frame,
                )
            };

            // Send the samples back; ignore send error (recorder may have stopped)
            let _ = response_tx.send(samples);
            continue;
        }

        // Compute batch size from current sample rate and batch interval
        let batch_size = (state.sample_rate as f64 * batch_interval.as_secs_f64()) as usize;
        let batch_size = batch_size.max(1);

        let gen_start = Instant::now();

        // Generate a batch of samples
        let samples = state.input.generate_samples_fixed(
            state.focus,
            state.aspect(),
            state.viewport_width,
            state.sample_rate,
            batch_size,
        );

        // Push into ring buffer (partial write if buffer is near-full)
        let pushed = if !samples.is_empty() {
            producer.push_bulk(&samples)
        } else {
            0
        };

        // Track drops
        let dropped = samples.len().saturating_sub(pushed);
        if dropped > 0 {
            stats
                .samples_dropped
                .fetch_add(dropped as u32, Ordering::Relaxed);
            tracing::warn!(dropped, "samples dropped (ring buffer full)");
        }

        // Update stats
        samples_this_second += pushed;
        generated_this_second += batch_size;
        stats
            .batch_interval
            .store(batch_interval.as_secs_f32(), Ordering::Relaxed);
        if second_timer.elapsed() >= Duration::from_secs(1) {
            let throughput = samples_this_second as f32;
            stats.throughput.store(throughput, Ordering::Relaxed);
            stats
                .samples_generated
                .store(generated_this_second as f32, Ordering::Relaxed);

            // If throughput fell below 90% of target, grow the batch interval
            // so each iteration produces more samples, amortizing loop overhead.
            if throughput < state.sample_rate * 0.9 {
                batch_interval = (batch_interval * 2).min(MAX_BATCH_INTERVAL);
            }

            samples_this_second = 0;
            generated_this_second = 0;
            second_timer = Instant::now();
        }

        let gen_elapsed = gen_start.elapsed();

        // Adaptive batch interval:
        // If generation took >80% of the batch interval, double it (up to cap).
        // If generation took <20% of the batch interval, halve it (down to floor).
        if gen_elapsed > batch_interval.mul_f32(0.8) {
            batch_interval = (batch_interval * 2).min(MAX_BATCH_INTERVAL);
        } else if gen_elapsed < batch_interval.mul_f32(0.2) {
            batch_interval = (batch_interval / 2).max(MIN_BATCH_INTERVAL);
        }

        // Pace to target interval
        next_tick += batch_interval;
        let now = Instant::now();
        if next_tick > now {
            spin_sleep::sleep(next_tick - now);
        } else {
            // Fell behind — reset to avoid burst catch-up
            next_tick = now;
        }
    }
}

/// Spawn the simulation thread. Returns a join handle and command sender.
pub fn spawn_simulation(
    producer: SampleProducer,
    stats: Arc<SimStats>,
) -> (
    thread::JoinHandle<()>,
    crossbeam_channel::Sender<SimCommand>,
) {
    let (tx, rx) = crossbeam_channel::unbounded();
    let handle = thread::Builder::new()
        .name("phosphor-sim".into())
        .spawn(move || {
            run_simulation(producer, rx, stats);
        })
        .expect("failed to spawn simulation thread");
    (handle, tx)
}
