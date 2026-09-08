//! `TaskStore`: every verb the CLI exposes, enforced at this layer.
//!
//! Write verbs run inside a transaction that holds the write lock from
//! its first statement (gate check + row update + event insert land
//! together or not at all; see `take_write_lock`). Events carry the dual
//! actor position: `actor` = who triggered the verb, `assignee` = who
//! executes the work at that moment.

use std::path::Path;

use crate::{Task, TaskEvent};
use kallip_blob_store::{BlobId, BlobStore};
use sea_orm::entity::prelude::*;
use sea_orm::{
    ActiveValue::Set, ConnectOptions, Database, DatabaseBackend, DatabaseConnection,
    DatabaseTransaction, QueryOrder, Statement, TransactionError, TransactionTrait,
};
use sea_orm_migration::MigratorTrait as _;
use serde::Serialize;
use time::OffsetDateTime;

use crate::entities::task::{ActiveModel, Column as TaskColumn, Entity as TaskEntity};
use crate::entities::task_event::{ActiveModel as EventActive, Entity as EventEntity};
use crate::entities::{task, task_event};
use crate::gates;
use crate::model::{ClosedReason, TaskStatus, Transition};
use crate::{Error, archive};

/// One task plus its event trail, shaped for the machine face (export).
#[derive(Serialize)]
pub struct TaskExport {
    pub id: i64,
    pub title: String,
    pub status: String,
    pub creator: Option<String>,
    pub assignee: Option<String>,
    pub seats: Vec<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    pub waiting: bool,
    pub waiting_since: Option<String>,
    pub closed_reason: Option<String>,
    pub close_summary: Option<String>,
    pub association: Option<AssociationExport>,
    /// Two-phase pointer: live path while open; after close, the content
    /// address (`archive_hash`) is the frozen truth. Both are exported.
    pub dossier_path: Option<String>,
    pub archive_hash: Option<String>,
    pub events: Vec<EventExport>,
}

/// Association keys (K8s involvedObject shape): message windows the
/// task lives in — an inbox id range and/or a lesche room + seq range.
#[derive(Serialize)]
pub struct AssociationExport {
    pub inbox_id_start: Option<i64>,
    pub inbox_id_end: Option<i64>,
    pub room_id: Option<String>,
    pub room_seq_start: Option<i64>,
    pub room_seq_end: Option<i64>,
}

#[derive(Serialize)]
pub struct EventExport {
    pub id: i64,
    pub kind: String,
    pub name: String,
    pub actor: Option<String>,
    pub assignee: Option<String>,
    pub from_status: Option<String>,
    pub to_status: Option<String>,
    pub payload: Option<serde_json::Value>,
    pub created_at: Option<String>,
}

/// Registration spec for a new task (dispatch-time metadata).
#[derive(Debug, Clone, Default)]
pub struct CreateSpec {
    pub title: String,
    pub creator: String,
    pub assignee: Option<String>,
    pub seats: Vec<String>,
    pub dossier_path: Option<String>,
    pub inbox_id_start: Option<i64>,
    pub inbox_id_end: Option<i64>,
    pub room_id: Option<String>,
    pub room_seq_start: Option<i64>,
    pub room_seq_end: Option<i64>,
}

/// One checkpoint call: a work-log action, optionally filing a review
/// receipt, optionally moving the machine to `review`, optionally toggling
/// the `waiting` timing marker.
#[derive(Debug, Clone, Default)]
pub struct CheckpointSpec {
    pub id: i64,
    pub actor: String,
    pub note: Option<String>,
    pub receipt: bool,
    pub review: bool,
    /// Some(true) = set the marker, Some(false) = clear it, None = leave it.
    pub waiting: Option<bool>,
}

#[derive(Debug, Clone, Default)]
pub struct TaskFilter {
    pub status: Option<TaskStatus>,
    pub assignee: Option<String>,
}

/// SQLite-backed task store.
#[derive(Clone)]
pub struct TaskStore {
    db: DatabaseConnection,
}

impl TaskStore {
    pub async fn open(path: &Path) -> Result<Self, Error> {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|e| {
                Error::Other(format!("create task db dir {}: {e}", parent.display()))
            })?;
        }
        let url = format!("sqlite://{}?mode=rwc", path.display());
        let mut opts = ConnectOptions::new(url);
        opts.max_connections(4);
        opts.map_sqlx_sqlite_opts(|o| {
            o.journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
                .synchronous(sqlx::sqlite::SqliteSynchronous::Normal)
                .busy_timeout(std::time::Duration::from_secs(5))
        });
        let db = Database::connect(opts).await?;
        crate::migration::Migrator::up(&db, None).await?;
        Ok(Self { db })
    }

    /// In-memory constructor for tests and scratch use: one connection,
    /// no on-disk file, migrations applied.
    pub async fn open_in_memory() -> Self {
        let mut opts = ConnectOptions::new("sqlite::memory:".to_owned());
        opts.max_connections(1);
        let db = Database::connect(opts).await.expect("in-memory db");
        crate::migration::Migrator::up(&db, None)
            .await
            .expect("migrations");
        Self { db }
    }

    /// Registers a task in the queue (`queued`): dispatch metadata lands on
    /// the row now, so the close gate can hold it accountable later. The
    /// row and its `create` event land in one transaction, so the event
    /// trail can always derive the flat table.
    pub async fn create(&self, spec: CreateSpec) -> Result<Task, Error> {
        validate_association(&spec)?;
        let now = now();
        let seats = serde_json::to_string(&spec.seats)?;
        let actor = spec.creator.clone();
        for attempt in 0..=BUSY_RETRIES {
            let spec = spec.clone();
            let seats = seats.clone();
            let actor = actor.clone();
            match self
                .db
                .transaction(|tx| {
                    Box::pin(async move {
                        take_write_lock(tx).await?;
                        let row = ActiveModel {
                            title: Set(spec.title),
                            status: Set(TaskStatus::Queued.as_str().to_string()),
                            creator: Set(Some(actor.clone())),
                            assignee: Set(spec.assignee.clone()),
                            seats: Set(seats),
                            created_at: Set(now),
                            updated_at: Set(now),
                            dossier_path: Set(spec.dossier_path),
                            inbox_id_start: Set(spec.inbox_id_start),
                            inbox_id_end: Set(spec.inbox_id_end),
                            room_id: Set(spec.room_id),
                            room_seq_start: Set(spec.room_seq_start),
                            room_seq_end: Set(spec.room_seq_end),
                            ..Default::default()
                        };
                        let inserted = TaskEntity::insert(row).exec(tx).await?;
                        append_event_tx(
                            tx,
                            inserted.last_insert_id,
                            "action",
                            "create",
                            Some(actor),
                            spec.assignee,
                            None,
                            None,
                            None,
                            now,
                        )
                        .await?;
                        load(tx, inserted.last_insert_id).await
                    })
                })
                .await
            {
                Err(TransactionError::Connection(ref e))
                    if attempt < BUSY_RETRIES && is_busy_conn(e) =>
                {
                    continue;
                }
                Err(TransactionError::Transaction(ref e))
                    if attempt < BUSY_RETRIES && is_busy_err(e) =>
                {
                    continue;
                }
                other => return other.map_err(flat_txn),
            }
        }
        unreachable!("busy retries are bounded")
    }

    /// Picks a queued task up (`queued -> in_progress`). The serial gate
    /// holds an assignee to one `in_progress` task; `--force` escapes with
    /// an auditable `force_start` event.
    pub async fn start(&self, id: i64, actor: &str, force: bool) -> Result<Task, Error> {
        let actor = actor.to_owned();
        for attempt in 0..=BUSY_RETRIES {
            let actor = actor.clone();
            match self
                .db
                .transaction(|tx| {
                    Box::pin(async move {
                        take_write_lock(tx).await?;
                        let row = load(tx, id).await?;
                        let status = parse_status(&row)?;
                        check_transition(Transition::Start, status, id)?;
                        // Pickup assigns the task to the picker when nobody was
                        // named at dispatch.
                        let assignee = row.assignee.clone().unwrap_or_else(|| actor.to_string());

                        if let Some((blocked_by, title)) =
                            gates::serial_gate_blocked(tx, &assignee, row.id).await?
                        {
                            if !force {
                                return Err(Error::SerialGate {
                                    assignee,
                                    blocked_by,
                                    title,
                                });
                            }
                            let payload = serde_json::json!({
                                "gate": "serial",
                                "blocked_by": blocked_by,
                                "blocked_title": title,
                            });
                            append_event_tx(
                                tx,
                                row.id,
                                "action",
                                "force_start",
                                Some(actor.to_string()),
                                Some(assignee.clone()),
                                None,
                                None,
                                Some(payload.to_string()),
                                now(),
                            )
                            .await?;
                        }

                        let update = ActiveModel {
                            id: Set(row.id),
                            status: Set(Transition::Start.to_str().to_string()),
                            assignee: Set(Some(assignee.clone())),
                            started_at: Set(row.started_at.or(Some(now()))),
                            updated_at: Set(now()),
                            ..Default::default()
                        };
                        update.update(tx).await?;
                        append_event_tx(
                            tx,
                            row.id,
                            "transition",
                            "start",
                            Some(actor.to_string()),
                            Some(assignee),
                            Some(status.as_str().to_string()),
                            Some(Transition::Start.to_str().to_string()),
                            None,
                            now(),
                        )
                        .await?;
                        load(tx, id).await
                    })
                })
                .await
            {
                Err(TransactionError::Connection(ref e))
                    if attempt < BUSY_RETRIES && is_busy_conn(e) =>
                {
                    continue;
                }
                Err(TransactionError::Transaction(ref e))
                    if attempt < BUSY_RETRIES && is_busy_err(e) =>
                {
                    continue;
                }
                other => return other.map_err(flat_txn),
            }
        }
        unreachable!("busy retries are bounded")
    }

    /// Records a checkpoint action; `--receipt` files a review receipt;
    /// `--review` moves the machine `in_progress -> review`; `--waiting` /
    /// `--no-waiting` toggle the timing marker (a marker, never a state).
    pub async fn checkpoint(&self, op: CheckpointSpec) -> Result<Task, Error> {
        for attempt in 0..=BUSY_RETRIES {
            let op = op.clone();
            match self
                .db
                .transaction(|tx| {
                    Box::pin(async move {
                        take_write_lock(tx).await?;
                        let row = load(tx, op.id).await?;
                        let status = parse_status(&row)?;
                        match status {
                            TaskStatus::InProgress | TaskStatus::Review => {}
                            _ => {
                                return Err(Error::InvalidTransition {
                                    id: row.id,
                                    from: status.as_str().to_string(),
                                    action: "checkpoint".to_string(),
                                    expected: "in_progress|review".to_string(),
                                });
                            }
                        }

                        if op.review {
                            if status != TaskStatus::InProgress {
                                return Err(Error::InvalidTransition {
                                    id: row.id,
                                    from: status.as_str().to_string(),
                                    action: "review".to_string(),
                                    expected: Transition::Review.legal_from_str(),
                                });
                            }
                            let update = ActiveModel {
                                id: Set(row.id),
                                status: Set(Transition::Review.to_str().to_string()),
                                updated_at: Set(now()),
                                ..Default::default()
                            };
                            update.update(tx).await?;
                            append_event_tx(
                                tx,
                                row.id,
                                "transition",
                                "review",
                                Some(op.actor.clone()),
                                row.assignee.clone(),
                                Some(status.as_str().to_string()),
                                Some(Transition::Review.to_str().to_string()),
                                None,
                                now(),
                            )
                            .await?;
                        }

                        if let Some(waiting) = op.waiting {
                            let update = ActiveModel {
                                id: Set(row.id),
                                waiting: Set(i64::from(waiting)),
                                waiting_since: Set(waiting.then_some(now())),
                                updated_at: Set(now()),
                                ..Default::default()
                            };
                            update.update(tx).await?;
                            let name = if waiting {
                                "waiting_set"
                            } else {
                                "waiting_clear"
                            };
                            append_event_tx(
                                tx,
                                row.id,
                                "action",
                                name,
                                Some(op.actor.clone()),
                                row.assignee.clone(),
                                None,
                                None,
                                None,
                                now(),
                            )
                            .await?;
                        }

                        if op.receipt {
                            let payload = note_payload(&op.note);
                            append_event_tx(
                                tx,
                                row.id,
                                "action",
                                "receipt",
                                Some(op.actor.clone()),
                                row.assignee.clone(),
                                None,
                                None,
                                payload,
                                now(),
                            )
                            .await?;
                        }

                        // The note event: skip when this call only toggled the
                        // waiting marker (its event already went in above).
                        if !op.receipt
                            && (op.note.is_some() || (!op.review && op.waiting.is_none()))
                        {
                            let payload = note_payload(&op.note);
                            append_event_tx(
                                tx,
                                row.id,
                                "action",
                                "checkpoint",
                                Some(op.actor.clone()),
                                row.assignee.clone(),
                                None,
                                None,
                                payload,
                                now(),
                            )
                            .await?;
                        }
                        load(tx, op.id).await
                    })
                })
                .await
            {
                Err(TransactionError::Connection(ref e))
                    if attempt < BUSY_RETRIES && is_busy_conn(e) =>
                {
                    continue;
                }
                Err(TransactionError::Transaction(ref e))
                    if attempt < BUSY_RETRIES && is_busy_err(e) =>
                {
                    continue;
                }
                other => return other.map_err(flat_txn),
            }
        }
        unreachable!("busy retries are bounded")
    }

    /// Closes a task. The close gate requires a receipt from every seat
    /// registered at dispatch in the current review cycle; `--force`
    /// escapes with an auditable event. The dossier (when registered) is
    /// packed canonically and ingested into the blob store before the
    /// transaction opens; the transaction then writes the pointer next to
    /// the gate check, the state flip, and the close event. Packing before
    /// the transaction trades a dossier-change window (TOCTOU) for a
    /// short write lock: the hash is the content address of what was
    /// actually archived, so the pointer is exact for that snapshot even
    /// if the live directory moves on afterwards.
    pub async fn close(
        &self,
        id: i64,
        actor: &str,
        reason: ClosedReason,
        summary: Option<String>,
        force: bool,
        blobs: Option<std::sync::Arc<dyn BlobStore>>,
    ) -> Result<Task, Error> {
        let actor = actor.to_owned();
        // Pack and ingest outside the write transaction: a large dossier
        // would otherwise hold the write lock for the whole archive IO.
        // TOCTOU: the live directory may change between this pack and
        // the commit below. The hash is the content address of the
        // packed snapshot, so the pointer stays exact for what was
        // archived; a later reopen + close archives fresh content under
        // a new hash.
        let row = self.load(id).await?;
        let archive_hash: Option<String> = match (&row.dossier_path, blobs.as_ref()) {
            (Some(dossier), Some(blobs)) => {
                let dir = Path::new(dossier);
                if !dir.is_dir() {
                    return Err(Error::DossierNotDir {
                        path: dossier.clone(),
                    });
                }
                let packed = archive::pack_dir(dir)?;
                let blob_id = archive::ingest(blobs.as_ref(), packed).await?;
                Some(blob_id.as_str().to_string())
            }
            (Some(dossier), None) => {
                return Err(Error::ArchiveNoBlobStore {
                    id,
                    path: dossier.clone(),
                });
            }
            _ => None,
        };
        for attempt in 0..=BUSY_RETRIES {
            let actor = actor.clone();
            let summary = summary.clone();
            let archive_hash = archive_hash.clone();
            match self
                .db
                .transaction(|tx| {
                    Box::pin(async move {
                        take_write_lock(tx).await?;
                        let row = load(tx, id).await?;
                        let status = parse_status(&row)?;
                        check_transition(Transition::Close, status, id)?;

                        let seats: Vec<String> = serde_json::from_str(&row.seats)?;
                        if !force {
                            let missing = gates::missing_receipts(tx, row.id, &seats).await?;
                            if !missing.is_empty() {
                                return Err(Error::ReceiptGate {
                                    missing: missing.join(", "),
                                });
                            }
                        } else {
                            let payload = serde_json::json!({
                                "gate": "receipts",
                                "registered_seats": seats,
                            });
                            append_event_tx(
                                tx,
                                row.id,
                                "action",
                                "force_close",
                                Some(actor.to_string()),
                                row.assignee.clone(),
                                None,
                                None,
                                Some(payload.to_string()),
                                now(),
                            )
                            .await?;
                        }

                        let update = ActiveModel {
                            id: Set(row.id),
                            status: Set(Transition::Close.to_str().to_string()),
                            ended_at: Set(Some(now())),
                            updated_at: Set(now()),
                            closed_reason: Set(Some(reason.as_str().to_string())),
                            close_summary: Set(summary.clone()),
                            archive_hash: Set(archive_hash.clone()),
                            waiting: Set(0),
                            waiting_since: Set(None),
                            ..Default::default()
                        };
                        update.update(tx).await?;

                        let mut payload = serde_json::json!({ "reason": reason.as_str() });
                        if let Some(summary) = &summary {
                            payload["summary"] = serde_json::Value::String(summary.clone());
                        }
                        if let Some(hash) = &archive_hash {
                            payload["archive_hash"] = serde_json::Value::String(hash.clone());
                        }
                        append_event_tx(
                            tx,
                            row.id,
                            "transition",
                            "close",
                            Some(actor.to_string()),
                            row.assignee.clone(),
                            Some(status.as_str().to_string()),
                            Some(Transition::Close.to_str().to_string()),
                            Some(payload.to_string()),
                            now(),
                        )
                        .await?;
                        load(tx, id).await
                    })
                })
                .await
            {
                Err(TransactionError::Connection(ref e))
                    if attempt < BUSY_RETRIES && is_busy_conn(e) =>
                {
                    continue;
                }
                Err(TransactionError::Transaction(ref e))
                    if attempt < BUSY_RETRIES && is_busy_err(e) =>
                {
                    continue;
                }
                other => return other.map_err(flat_txn),
            }
        }
        unreachable!("busy retries are bounded")
    }

    /// Reworks a closed task (`closed -> in_progress`). The archive stays
    /// content-addressed (unchanged content keeps its hash); the next close
    /// writes a fresh pointer for changed content. History is in the events.
    /// Reopening runs the serial gate like `start` — an assignee still
    /// works one task at a time (`--force` escapes with an auditable
    /// event) — and the `reopen` event doubles as the receipt-cycle
    /// marker: prior-cycle receipts no longer satisfy the close gate.
    pub async fn reopen(&self, id: i64, actor: &str, force: bool) -> Result<Task, Error> {
        let actor = actor.to_owned();
        for attempt in 0..=BUSY_RETRIES {
            let actor = actor.clone();
            match self
                .db
                .transaction(|tx| {
                    Box::pin(async move {
                        take_write_lock(tx).await?;
                        let row = load(tx, id).await?;
                        let status = parse_status(&row)?;
                        check_transition(Transition::Reopen, status, id)?;
                        if let Some(assignee) = row.assignee.as_deref()
                            && let Some((blocked_by, title)) =
                                gates::serial_gate_blocked(tx, assignee, row.id).await?
                        {
                            if !force {
                                return Err(Error::SerialGate {
                                    assignee: assignee.to_string(),
                                    blocked_by,
                                    title,
                                });
                            }
                            let payload = serde_json::json!({
                                "gate": "serial",
                                "blocked_by": blocked_by,
                                "blocked_title": title,
                            });
                            append_event_tx(
                                tx,
                                row.id,
                                "action",
                                "force_start",
                                Some(actor.to_string()),
                                row.assignee.clone(),
                                None,
                                None,
                                Some(payload.to_string()),
                                now(),
                            )
                            .await?;
                        }

                        let update = ActiveModel {
                            id: Set(row.id),
                            status: Set(Transition::Reopen.to_str().to_string()),
                            ended_at: Set(None),
                            updated_at: Set(now()),
                            closed_reason: Set(None),
                            close_summary: Set(None),
                            waiting: Set(0),
                            waiting_since: Set(None),
                            ..Default::default()
                        };
                        update.update(tx).await?;
                        append_event_tx(
                            tx,
                            row.id,
                            "transition",
                            "reopen",
                            Some(actor.to_string()),
                            row.assignee.clone(),
                            Some(status.as_str().to_string()),
                            Some(Transition::Reopen.to_str().to_string()),
                            None,
                            now(),
                        )
                        .await?;
                        load(tx, id).await
                    })
                })
                .await
            {
                Err(TransactionError::Connection(ref e))
                    if attempt < BUSY_RETRIES && is_busy_conn(e) =>
                {
                    continue;
                }
                Err(TransactionError::Transaction(ref e))
                    if attempt < BUSY_RETRIES && is_busy_err(e) =>
                {
                    continue;
                }
                other => return other.map_err(flat_txn),
            }
        }
        unreachable!("busy retries are bounded")
    }

    pub async fn list(&self, filter: TaskFilter) -> Result<Vec<Task>, Error> {
        let mut query = TaskEntity::find();
        if let Some(status) = filter.status {
            query = query.filter(TaskColumn::Status.eq(status.as_str()));
        }
        if let Some(assignee) = filter.assignee {
            query = query.filter(TaskColumn::Assignee.eq(assignee));
        }
        Ok(query.order_by_asc(TaskColumn::Id).all(&self.db).await?)
    }

    pub async fn get(&self, id: i64) -> Result<(Task, Vec<TaskEvent>), Error> {
        let task = self.load(id).await?;
        let events = self.events_of(id).await?;
        Ok((task, events))
    }

    pub async fn events_of(&self, id: i64) -> Result<Vec<TaskEvent>, Error> {
        Ok(EventEntity::find()
            .filter(task_event::Column::TaskId.eq(id))
            .order_by_asc(task_event::Column::Id)
            .all(&self.db)
            .await?)
    }

    /// The machine face: the task plus its trail, stable field names, ISO
    /// 8601 UTC times.
    pub async fn export(&self, id: i64) -> Result<TaskExport, Error> {
        let (task, events) = self.get(id).await?;
        to_export(task, events)
    }

    /// Content address of a closed archive, for `task extract`.
    pub fn archive_blob_id(task: &Task) -> Result<Option<BlobId>, Error> {
        task.archive_hash
            .as_deref()
            .map(BlobId::parse)
            .transpose()
            .map_err(Error::from)
    }

    async fn load(&self, id: i64) -> Result<Task, Error> {
        TaskEntity::find_by_id(id)
            .one(&self.db)
            .await?
            .ok_or(Error::NotFound { id })
    }
}

fn to_export(task: Task, events: Vec<TaskEvent>) -> Result<TaskExport, Error> {
    let iso = |secs: Option<i64>| {
        secs.map(crate::model::iso8601_utc)
            .transpose()
            .map_err(Error::from)
    };
    let seats: Vec<String> = serde_json::from_str(&task.seats)?;
    let association =
        if task.inbox_id_start.is_some() || task.inbox_id_end.is_some() || task.room_id.is_some() {
            Some(AssociationExport {
                inbox_id_start: task.inbox_id_start,
                inbox_id_end: task.inbox_id_end,
                room_id: task.room_id.clone(),
                room_seq_start: task.room_seq_start,
                room_seq_end: task.room_seq_end,
            })
        } else {
            None
        };
    let mut event_exports = Vec::with_capacity(events.len());
    for e in events {
        event_exports.push(EventExport {
            id: e.id,
            kind: e.kind.clone(),
            name: e.name.clone(),
            actor: e.actor.clone(),
            assignee: e.assignee.clone(),
            from_status: e.from_status.clone(),
            to_status: e.to_status.clone(),
            payload: e.payload.as_deref().map(serde_json::from_str).transpose()?,
            created_at: iso(Some(e.created_at))?,
        });
    }
    Ok(TaskExport {
        id: task.id,
        title: task.title.clone(),
        status: task.status.clone(),
        creator: task.creator.clone(),
        assignee: task.assignee.clone(),
        seats,
        created_at: iso(Some(task.created_at))?,
        updated_at: iso(Some(task.updated_at))?,
        started_at: iso(task.started_at)?,
        ended_at: iso(task.ended_at)?,
        waiting: task.waiting != 0,
        waiting_since: iso(task.waiting_since)?,
        closed_reason: task.closed_reason.clone(),
        close_summary: task.close_summary.clone(),
        association,
        dossier_path: task.dossier_path.clone(),
        archive_hash: task.archive_hash.clone(),
        events: event_exports,
    })
}

async fn load(tx: &DatabaseTransaction, id: i64) -> Result<task::Model, Error> {
    task::Entity::find_by_id(id)
        .one(tx)
        .await?
        .ok_or(Error::NotFound { id })
}

fn parse_status(row: &task::Model) -> Result<TaskStatus, Error> {
    TaskStatus::parse(&row.status).ok_or_else(|| {
        Error::Other(format!(
            "task {} carries unknown status '{}'",
            row.id, row.status
        ))
    })
}

fn check_transition(t: Transition, from: TaskStatus, id: i64) -> Result<(), Error> {
    if t.legal_from().contains(&from) {
        Ok(())
    } else {
        Err(Error::InvalidTransition {
            id,
            from: from.as_str().to_string(),
            action: t.name().to_string(),
            expected: t.legal_from_str(),
        })
    }
}

fn note_payload(note: &Option<String>) -> Option<String> {
    note.as_ref()
        .map(|n| serde_json::json!({ "note": n }).to_string())
}

#[allow(clippy::too_many_arguments)]
async fn append_event_tx(
    db: &impl sea_orm::ConnectionTrait,
    task_id: i64,
    kind: &str,
    name: &str,
    actor: Option<String>,
    assignee: Option<String>,
    from_status: Option<String>,
    to_status: Option<String>,
    payload: Option<String>,
    created_at: i64,
) -> Result<(), Error> {
    let event = EventActive {
        task_id: Set(task_id),
        kind: Set(kind.to_string()),
        name: Set(name.to_string()),
        actor: Set(actor),
        assignee: Set(assignee),
        from_status: Set(from_status),
        to_status: Set(to_status),
        payload: Set(payload),
        created_at: Set(created_at),
        ..Default::default()
    };
    EventEntity::insert(event).exec(db).await?;
    Ok(())
}

fn flat_txn(e: TransactionError<Error>) -> Error {
    match e {
        TransactionError::Connection(e) => Error::Db(e),
        TransactionError::Transaction(e) => e,
    }
}

/// Takes the SQLite write lock at the start of a just-begun deferred
/// transaction — the `BEGIN IMMEDIATE` semantics sea-orm/sqlx do not
/// expose for SQLite. The first statement is a zero-row UPDATE: it
/// grabs the RESERVED write lock before any snapshot read, so gate
/// checks run under the lock and concurrent writers queue on
/// `busy_timeout` instead of failing on a stale-snapshot upgrade
/// (SQLITE_BUSY_SNAPSHOT), which `busy_timeout` cannot cover.
async fn take_write_lock(tx: &DatabaseTransaction) -> Result<(), Error> {
    tx.execute(Statement::from_string(
        DatabaseBackend::Sqlite,
        "UPDATE tasks SET updated_at = updated_at WHERE id = -1",
    ))
    .await
    .map_err(Error::from)?;
    Ok(())
}

/// Whole-transaction retries for busy-class write failures. The write
/// lock is taken by the transaction's first statement, so a busy is a
/// lock wait that outlived `busy_timeout`, never a half-applied state:
/// a retry re-runs the gate checks against fresh state. Non-busy
/// errors return immediately.
const BUSY_RETRIES: usize = 3;

fn is_busy_err(e: &Error) -> bool {
    let s = e.to_string();
    s.contains("database is locked") || s.contains("database table is locked")
}

fn is_busy_conn(e: &sea_orm::DbErr) -> bool {
    let s = e.to_string();
    s.contains("database is locked") || s.contains("database table is locked")
}

/// Association keys are paired windows: each range is either fully
/// absent or fully present with `start <= end`, and the seq range
/// needs the room it belongs to. One-sided or inverted windows would
/// silently narrow the trail the task claims (`show` hides one-sided
/// windows outright).
fn validate_association(spec: &CreateSpec) -> Result<(), Error> {
    fn window(start: Option<i64>, end: Option<i64>, what: &str) -> Result<(), Error> {
        match (start, end) {
            (None, None) => Ok(()),
            (Some(s), Some(e)) if s <= e => Ok(()),
            (Some(s), Some(e)) => Err(Error::AssociationInvalid {
                detail: format!("{what} window {s}..{e} is inverted"),
            }),
            _ => Err(Error::AssociationInvalid {
                detail: format!("{what} window needs both bounds"),
            }),
        }
    }
    window(spec.inbox_id_start, spec.inbox_id_end, "inbox id")?;
    window(spec.room_seq_start, spec.room_seq_end, "room seq")?;
    if spec.room_id.is_none() && spec.room_seq_start.is_some() {
        return Err(Error::AssociationInvalid {
            detail: "room seq window needs the room id it belongs to".to_string(),
        });
    }
    Ok(())
}

fn now() -> i64 {
    OffsetDateTime::now_utc().unix_timestamp()
}
