//! Thin binary: parse config, resolve the auth mode, serve.

use anyhow::{Context, Result};
use clap::Parser as _;
use kallip_daemon_client::DaemonClient;
use kallip_instances::backend::UdsBackend;
use kallip_instances::{AppState, Config, build_router, resolve_auth};

#[tokio::main]
async fn main() -> Result<()> {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let config = Config::parse();
    let socket = config.resolve_socket()?;
    let addr = config.addr.clone();

    // Resolve the auth mode from the configuration. Fail-safe rule: a
    // non-loopback bind with neither platform nor standalone credentials
    // refuses to start — the open mode is a loopback-only convenience.
    let auth = resolve_auth(&config, &addr)?;
    let state = AppState {
        backend: UdsBackend::arc(DaemonClient::new(socket.clone())),
        auth,
        allowed_hosts: config.allowed_hosts(),
        cors_origins: config.cors_origins.clone(),
    };
    let app = build_router(state, config.static_dir.as_deref());

    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .with_context(|| format!("binding {addr}"))?;
    tracing::info!(
        addr = %addr,
        socket = %socket.display(),
        "kallip-instances listening"
    );
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = tokio::signal::ctrl_c();
    let sigterm = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };
    tokio::select! {
        _ = ctrl_c => {},
        _ = sigterm => {},
    }
}
