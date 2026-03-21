#![allow(dead_code)]

use clap::Parser;

mod app;
mod audio_output;
mod beam;
mod cli;
mod controls_window;
mod frame;
mod gpu;
mod phosphor;
mod presets;
mod recording;
mod simulation;
mod simulation_stats;
mod types;
mod ui;

fn main() -> anyhow::Result<()> {
    let cli = cli::Cli::parse();

    let env_filter = tracing_subscriber::EnvFilter::builder()
        .with_default_directive("phosphor=info".parse()?)
        .from_env()?;

    if cli.record.is_some() {
        // CLI mode: use tracing-indicatif so log lines don't stomp the progress bar
        use tracing_subscriber::layer::SubscriberExt;
        use tracing_subscriber::util::SubscriberInitExt;

        let indicatif_layer = tracing_indicatif::IndicatifLayer::new();
        let writer = indicatif_layer.get_stderr_writer();
        tracing_subscriber::registry()
            .with(env_filter)
            .with(tracing_subscriber::fmt::layer().with_writer(writer))
            .with(indicatif_layer)
            .init();

        cli::run_headless(&cli)
    } else {
        // UI mode: standard stderr logging
        let (non_blocking, _guard) = tracing_appender::non_blocking(std::io::stderr());
        tracing_subscriber::fmt()
            .with_env_filter(env_filter)
            .with_writer(non_blocking)
            .init();

        let event_loop = winit::event_loop::EventLoop::new().expect("failed to create event loop");
        let mut app = app::App::default();
        event_loop.run_app(&mut app).expect("event loop error");
        Ok(())
    }
}
