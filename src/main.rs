#![allow(dead_code)]

mod app;
mod beam;
mod controls_window;
mod frame;
mod gpu;
mod phosphor;
mod presets;
mod simulation;
mod simulation_stats;
mod types;
mod ui;

fn main() -> anyhow::Result<()> {
    let (non_blocking, _guard) = tracing_appender::non_blocking(std::io::stderr());
    let env_filter = tracing_subscriber::EnvFilter::builder()
        .with_default_directive("phosphor=info".parse()?)
        .from_env()?;
    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_writer(non_blocking)
        .init();

    let event_loop = winit::event_loop::EventLoop::new().expect("failed to create event loop");
    let mut app = app::App::default();
    event_loop.run_app(&mut app).expect("event loop error");

    Ok(())
}
