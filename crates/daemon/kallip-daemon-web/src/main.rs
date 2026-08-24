//! Thin binary: parse config, maybe generate a token, serve.

use anyhow::{Context, Result};
use clap::Parser as _;
use kallip_daemon_client::DaemonClient;
use kallip_daemon_web::{AppState, Config, build_router};

#[tokio::main]
async fn main() -> Result<()> {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let config = Config::parse();
    let socket = config.resolve_socket();
    let addr = config.addr.clone();

    let token = match &config.token {
        Some(token) => token.clone(),
        None => {
            // Generated once at startup and printed once: it is the only
            // chance to copy it, and it never lands in persistent state.
            let mut bytes = [0u8; 16];
            getrandom::fill(&mut bytes).context("generate token entropy")?;
            let token = hex::encode(bytes);
            tracing::info!("generated management token (shown once): {token}");
            token
        }
    };

    let state = AppState {
        client: DaemonClient::new(socket.clone()),
        token,
    };
    let app = build_router(state, config.static_dir.as_deref());

    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .with_context(|| format!("binding {addr}"))?;
    tracing::info!(
        addr = %addr,
        socket = %socket.display(),
        "kallip-daemon-web listening"
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
