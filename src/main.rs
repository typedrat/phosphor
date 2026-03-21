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

    let (non_blocking, _guard) = tracing_appender::non_blocking(std::io::stderr());
    let env_filter = tracing_subscriber::EnvFilter::builder()
        .with_default_directive("phosphor=info".parse()?)
        .from_env()?;
    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_writer(non_blocking)
        .init();

    if cli.record.is_some() {
        cli::run_headless(&cli)
    } else {
        let event_loop = winit::event_loop::EventLoop::new().expect("failed to create event loop");
        let mut app = app::App::default();
        event_loop.run_app(&mut app).expect("event loop error");
        Ok(())
    }
}
