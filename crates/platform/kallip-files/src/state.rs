//! Service state and runtime assembly: configuration, the shared
//! [`AppState`], the router, and the boot sequence ([`run`]).

use std::error::Error;
use std::path::PathBuf;
use std::sync::Arc;

use crate::metadata::{self, Db};
use axum::Router;

use crate::api;
use crate::auth::FilesControlPlane;
use crate::backend::LocalBackend;
use crate::blob::BlobStore;
use crate::gc::GcConfig;
use kallip_agora_common::control_plane::ControlPlane;

/// Static service configuration, resolved once at boot.
#[derive(Debug, Clone)]
pub struct FilesConfig {
    /// Maximum accepted upload body, in bytes; a longer stream is cut off
    /// with 413. The value is a size ceiling only (Q1 default 100 MB; the
    /// operator may tune it), never a chunking boundary.
    pub max_body_bytes: u64,
    /// Agora degrade posture (seventh approved default). `false` (default)
    /// is fail-closed: a registry that cannot answer produces 503 and no
    /// decision. `true` is fail-soft: the enrollment lookup degrading to an
    /// empty fact set turns tagma decisions into denials (403) instead of
    /// 503s. It never weakens verification -- with the registry down, no
    /// request authenticates either way.
    pub degrade_fail_soft: bool,
    /// Garbage collection cadence (sweep + reconcile per tick; the first
    /// tick fires immediately, which is the startup audit).
    pub gc: GcConfig,
}

/// Shared state handed to every handler and extractor.
#[derive(Clone)]
pub struct AppState {
    /// Metadata database (migrated at boot).
    pub db: Db,
    /// The blob store behind the object-safe seam.
    pub blob: Arc<dyn BlobStore>,
    /// Blob root path, for the reconcile walk (the store trait has no
    /// directory listing; reconciliation is root-aware by design).
    pub blob_root: PathBuf,
    /// The agora control-plane client.
    pub control: Arc<dyn ControlPlane>,
    /// Static configuration.
    pub config: Arc<FilesConfig>,
}

/// Everything the service needs at boot. `main` fills it from CLI/env; the
/// integration smoke test fills it directly against a test Postgres.
pub struct BootConfig {
    /// Address to bind (behind a TLS-terminating reverse proxy).
    pub listen_addr: String,
    /// Postgres URL for the metadata store.
    pub database_url: String,
    /// Agora internal base URL for `/internal/*` calls.
    pub agora_internal_url: String,
    /// Shared secret bearer for the agora internal API.
    pub agora_internal_token: String,
    /// Root directory of the blob store (the reconciler walks it; the
    /// store trait itself has no directory listing).
    pub blob_root: std::path::PathBuf,
    /// The files-specific statics.
    pub files: FilesConfig,
}

/// The authenticated API surface. `/health` is a deliberate no-auth route
/// (compose healthcheck / Caddy and baseline acceptance; the instances'
/// health route is the precedent).
pub fn router(state: AppState) -> Router {
    Router::new()
        .route(
            "/v1/files",
            axum::routing::put(api::put::put_file).get(api::list::list_files),
        )
        .route(
            "/v1/files/{id}",
            axum::routing::get(api::get::get_file)
                .head(api::get::head_file)
                .delete(api::delete_file),
        )
        .route(
            "/v1/files/{id}/send",
            axum::routing::post(api::send::send_file),
        )
        .route(
            "/v1/admin/delivery-events",
            axum::routing::get(api::admin::list_events),
        )
        .route("/health", axum::routing::get(api::health))
        .with_state(state)
}

/// Boot sequence: connect and migrate the metadata store, build the state,
/// start the GC driver (its first tick is the startup audit), and serve.
/// Migration failure is fatal (fail fast, the plan's connect_and_migrate
/// posture) -- the caller sees the error and the process exits nonzero.
pub async fn run(boot: BootConfig) -> Result<(), Box<dyn Error + Send + Sync>> {
    let db = metadata::connect_and_migrate(&boot.database_url).await?;
    let state = AppState {
        db: db.clone(),
        blob: LocalBackend::arc(&boot.blob_root),
        blob_root: boot.blob_root.clone(),
        control: Arc::new(FilesControlPlane::new(
            boot.agora_internal_url,
            boot.agora_internal_token,
        )),
        config: Arc::new(boot.files),
    };
    spawn_gc_driver(state.clone());
    let listener = tokio::net::TcpListener::bind(&boot.listen_addr).await?;
    axum::serve(listener, router(state)).await?;
    Ok(())
}

/// Start the background GC driver: sweep + reconcile each interval tick.
/// The first `interval` tick fires immediately, so the driver doubles as
/// the startup audit (the plan's "startup + periodic" self-check). Drift
/// found by reconcile is a warning, never an error: reconciliation is a
/// detector, not a repairer (the GC module owns that distinction).
pub fn spawn_gc_driver(state: AppState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(state.config.gc.interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            match crate::gc::sweep(&state.db, state.blob.as_ref(), &state.config.gc).await {
                Ok(report) => {
                    if report.catalog_reclaimed > 0 {
                        tracing::info!(reclaimed = report.catalog_reclaimed, "gc sweep");
                    }
                }
                Err(e) => tracing::warn!(error = %e, "gc sweep failed"),
            }
            match crate::gc::reconcile(&state.db, &state.blob_root).await {
                Ok(report) => {
                    if !report.missing_blobs.is_empty() || !report.orphan_files.is_empty() {
                        tracing::warn!(
                            missing_blobs = report.missing_blobs.len(),
                            orphan_files = report.orphan_files.len(),
                            "catalog/store drift detected"
                        );
                    }
                }
                Err(e) => tracing::warn!(error = %e, "gc reconcile failed"),
            }
        }
    })
}
