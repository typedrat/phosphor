use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::Receiver;

use crate::app::{InputMode, InputState, OscilloscopeState};
use crate::beam::SampleProducer;
use crate::types::Resolution;

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
    SetAccumResolution(Resolution),
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
    accum_resolution: Resolution,
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
            accum_resolution: Resolution::new(800, 600),
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
            SimCommand::SetAccumResolution(res) => self.accum_resolution = res,
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
pub fn run_simulation(mut producer: SampleProducer, commands: Receiver<SimCommand>) {
    let _span = tracing::info_span!("sim").entered();
    let mut state = SimState::new();

    tracing::info!(sample_rate = state.sample_rate, "thread started");

    let mut batch_interval = MIN_BATCH_INTERVAL;
    let mut next_tick = Instant::now();

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
            state.accum_resolution,
            state.sample_rate,
            batch_size,
        );

        // Push into ring buffer (partial write if buffer is near-full)
        if !samples.is_empty() {
            producer.push_bulk(&samples);
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
) -> (
    thread::JoinHandle<()>,
    crossbeam_channel::Sender<SimCommand>,
) {
    let (tx, rx) = crossbeam_channel::unbounded();
    let handle = thread::Builder::new()
        .name("phosphor-sim".into())
        .spawn(move || {
            run_simulation(producer, rx);
        })
        .expect("failed to spawn simulation thread");
    (handle, tx)
}
