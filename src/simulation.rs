use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::Receiver;

use crate::beam::audio::AudioSource;
use crate::beam::oscilloscope::{ChannelConfig, OscilloscopeSource};
use crate::beam::vector::VectorSegment;
use crate::beam::{BeamSample, BeamSource, BeamState, SampleProducer};
use crate::simulation_stats::SimStats;
use crate::types::{ExternalState, InputMode, OscilloscopeState};

/// Calibration constant for beam energy deposition. The beam_write shader
/// computes `energy = intensity * profile * dt`, where dt is the per-sample
/// dwell time (~1/44100 s). Without scaling, the deposited energy is on the
/// order of 1e-5, which is invisible after spectral integration and
/// tonemapping. This constant represents the beam current / power scale
/// that makes the phosphor visibly glow at the default settings.
const BEAM_ENERGY_SCALE: f32 = 5000.0;

pub struct AudioState {
    pub file_path: Option<PathBuf>,
    pub source: Option<AudioSource>,
    pub playing: bool,
    pub looping: bool,
    pub speed: f32,
    pub load_error: Option<String>,
}

impl Default for AudioState {
    fn default() -> Self {
        Self {
            file_path: None,
            source: None,
            playing: false,
            looping: false,
            speed: 1.0,
            load_error: None,
        }
    }
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
                if !audio.playing {
                    return Vec::new();
                }
                let Some(source) = &mut audio.source else {
                    return Vec::new();
                };
                let adj_count = (count as f32 * audio.speed) as usize;
                if adj_count == 0 {
                    return Vec::new();
                }
                let samples = source.generate(adj_count, &beam);
                if source.is_finished() {
                    if audio.looping {
                        source.seek(0.0);
                    } else {
                        audio.playing = false;
                    }
                }
                samples
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

    pub fn load_audio_file(&mut self, path: PathBuf) {
        match AudioSource::load(&path) {
            Ok(source) => {
                self.audio.source = Some(source);
                self.audio.file_path = Some(path);
                self.audio.load_error = None;
                self.audio.playing = true;
            }
            Err(e) => {
                self.audio.load_error = Some(e.to_string());
                self.audio.source = None;
            }
        }
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
    LoadAudioFile(PathBuf),
    SetAudioPlaying(bool),
    SetAudioLooping(bool),
    SetAudioSpeed(f32),
    LoadVectorFile(PathBuf),
    /// Sample rate change — carries the new producer from a resized channel.
    /// The render thread creates the new channel and swaps its consumer.
    SetSampleRate {
        rate: f32,
        producer: SampleProducer,
    },
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
            SimCommand::LoadAudioFile(path) => self.input.load_audio_file(path),
            SimCommand::SetAudioPlaying(p) => self.input.audio.playing = p,
            SimCommand::SetAudioLooping(l) => self.input.audio.looping = l,
            SimCommand::SetAudioSpeed(s) => self.input.audio.speed = s,
            SimCommand::LoadVectorFile(path) => self.input.load_vector_file(path),
            SimCommand::SetSampleRate { rate, .. } => self.sample_rate = rate,
            SimCommand::Shutdown => {} // handled by caller
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
            state.apply_command(cmd);
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
