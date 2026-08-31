use clap::Parser;

/// CLI arguments for `kallip-files`, the file transfer service.
///
/// The service owns two stores: the content-addressed blob directory
/// (local volume; the cloud object-store backend is a postponed trait
/// sibling) and the metadata Postgres. Identity and enrollment facts stay
/// in the agora, reached through the `/internal/*` ControlPlane API.
#[derive(Parser)]
#[command(
    name = "kallip-files",
    version,
    about = "kallip file transfer service: content-addressed blobs, ACL'd spaces"
)]
pub struct Args {
    /// Address to listen on (behind a TLS-terminating reverse proxy).
    #[arg(long, env = "KALLIP_FILES_ADDR", default_value = "127.0.0.1:7300")]
    pub listen_addr: String,
    /// Root directory of the content-addressed blob store. Created on
    /// demand by the store itself.
    #[arg(long, env = "KALLIP_FILES_BLOB_ROOT")]
    pub blob_root: String,
    /// Agora internal base URL for `/internal/*` ControlPlane calls (e.g.
    /// `http://127.0.0.1:7100`). Must NOT be publicly reachable.
    #[arg(long, env = "KALLIP_FILES_AGORA_INTERNAL_URL")]
    pub agora_internal_url: String,
    /// Shared secret bearer for the agora `/internal/*` API. Must equal
    /// the agora's `KALLIP_AGORA_INTERNAL_TOKEN`.
    #[arg(long, env = "KALLIP_FILES_AGORA_TOKEN")]
    pub agora_internal_token: String,
    /// Postgres URL for the metadata store (e.g.
    /// `postgres://user:pass@host/db`). Required: records, refcounts, and
    /// the delivery log are the service's durable surface; a missing URL
    /// fails fast at boot rather than silently running with no store.
    #[arg(long, env = "KALLIP_FILES_DATABASE_URL")]
    pub database_url: String,
    /// Maximum accepted upload body, in megabytes. A larger stream is cut
    /// off with 413. The default is the plan's Q1 placeholder value; the
    /// operator may tune it (structure is unaffected).
    #[arg(long, env = "KALLIP_FILES_MAX_BODY_SIZE_MB", default_value_t = 100)]
    pub max_body_size_mb: u64,
    /// Agora degrade posture (seventh approved default). `closed` (the
    /// default) fails every authorization decision with 503 when the
    /// registry cannot answer; `soft` degrades the enrollment lookup to an
    /// empty fact set, so tagma decisions deny with 403 instead of 503.
    /// Neither posture weakens credential verification.
    #[arg(long, env = "KALLIP_FILES_DEGRADE", value_parser = ["closed", "soft"], default_value = "closed")]
    pub degrade: String,
    /// Delay between GC passes (sweep + reconcile), in seconds.
    #[arg(long, env = "KALLIP_FILES_GC_INTERVAL_SECS", default_value_t = 60)]
    pub gc_interval_secs: u64,
    /// How long a zero-refcount row must have been freed before the GC may
    /// reclaim it and unlink the file, in seconds.
    #[arg(long, env = "KALLIP_FILES_GC_GRACE_SECS", default_value_t = 60)]
    pub gc_grace_secs: u64,
    /// Maximum catalog rows reclaimed per GC pass.
    #[arg(long, env = "KALLIP_FILES_GC_BATCH", default_value_t = 128)]
    pub gc_batch: u32,
}
