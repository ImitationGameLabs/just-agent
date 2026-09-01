//! `kallip-files`: the kallip file transfer service.
//!
//! Content lives in a local content-addressed blob directory; records,
//! reference counts, and the delivery log live in the service's own
//! Postgres. Every request authenticates against the agora through the
//! `/internal/*` ControlPlane API (per request, no cache) and is
//! authorized by the single-point ACL (`acl`) against the two-layer space
//! namespace.

mod args;

use std::time::Duration;

use clap::Parser;

use crate::args::Args;
use kallip_files::gc;
use kallip_files::state::{BootConfig, FilesConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let args = Args::parse();

    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let config = FilesConfig {
        max_body_bytes: args.max_body_size_mb * 1024 * 1024,
        degrade_fail_soft: args.degrade == "soft",
        cors_origins: args.cors_origins,
        gc: gc::GcConfig {
            batch: args.gc_batch,
            interval: Duration::from_secs(args.gc_interval_secs),
            grace: Duration::from_secs(args.gc_grace_secs),
        },
    };
    let boot = BootConfig {
        listen_addr: args.listen_addr,
        database_url: args.database_url,
        agora_internal_url: args.agora_internal_url,
        agora_internal_token: args.agora_internal_token,
        blob_root: args.blob_root.into(),
        files: config,
    };

    tracing::info!("starting kallip-files");
    kallip_files::state::run(boot).await
}
