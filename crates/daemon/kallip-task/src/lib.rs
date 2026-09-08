//! The task ledger: a coarse state machine, an append-only event trail, two
//! hard gates, and closed-task content-addressed archives, over one SQLite
//! database (`tasks.sqlite`, sibling of the other tagma stores).
//!
//! Layering: this crate owns the data plane only. Storage roots and blob
//! backends are handed in by the caller (the CLI resolves
//! `KALLIP_TAGMA_DATA_DIR`), so tests can point everything at a scratch
//! directory. Gates are enforced inside the transition transaction, not at
//! the CLI surface — the CLI is an entry point, the store is the law.

pub mod archive;
pub mod entities;
pub mod gates;
pub mod migration;
pub mod model;
pub mod store;

pub use entities::task::Model as Task;
pub use entities::task_event::Model as TaskEvent;
pub use kallip_blob_store::BlobStore;
pub use model::{ClosedReason, EventKind, TaskStatus, Transition};
pub use store::{CheckpointSpec, CreateSpec, TaskExport, TaskFilter, TaskStore};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("task {id} not found")]
    NotFound { id: i64 },

    #[error("invalid transition: task {id} is {from}, {action} needs {expected}")]
    InvalidTransition {
        id: i64,
        from: String,
        action: String,
        expected: String,
    },

    #[error(
        "serial gate: assignee {assignee} already has task {blocked_by} in \
         progress ('{title}'); --force to override (escape is recorded)"
    )]
    SerialGate {
        assignee: String,
        blocked_by: i64,
        title: String,
    },

    #[error(
        "close gate: missing review receipts from registered seats: {missing}; \
         --force to override (escape is recorded)"
    )]
    ReceiptGate { missing: String },
    #[error(
        "archive gate: task {id} is {status}; only closed tasks archive; --force to override (escape is recorded)"
    )]
    ArchiveGate { id: i64, status: String },

    #[error(
        "dispatch gate: task {id} has no review dispatch this cycle; run `task dispatch` first, or --force to override (escape is recorded)"
    )]
    DispatchGate { id: i64 },

    #[error(
        "gate-report gate: task {id} has no gate report since its last recorded chain op; run `task gate-report` first, or --force to override (escape is recorded)"
    )]
    GateReportGate { id: i64 },

    #[error("dossier path {path} is not a directory")]
    DossierNotDir { path: String },
    #[error("task {id} registers a dossier ({path}) but close got no blob store to archive it")]
    ArchiveNoBlobStore { id: i64, path: String },

    #[error("invalid association keys: {detail}")]
    AssociationInvalid { detail: String },

    #[error(transparent)]
    Db(#[from] sea_orm::DbErr),

    #[error(transparent)]
    Blob(#[from] kallip_blob_store::Error),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Json(#[from] serde_json::Error),

    #[error(transparent)]
    Time(#[from] time::error::ComponentRange),

    #[error("{0}")]
    Other(String),
}
