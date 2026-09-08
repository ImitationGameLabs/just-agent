//! The two hard gates, enforced inside the transaction that performs the
//! gated transition.
//!
//! Serialization point: SQLite WAL. Every write transaction takes the
//! write lock with its first statement (see `store::take_write_lock`),
//! so the gate check reads under the lock: two concurrent gated
//! transitions queue on `busy_timeout` instead of racing — the loser
//! waits for the winner to commit, then re-checks the gate against the
//! fresh state.
//!
//! Close-gate receipts are cycle-scoped: reopening a task (the `reopen`
//! transition event) invalidates prior-cycle receipts, so the gate only
//! counts receipts filed after the most recent reopen.

use sea_orm::{ColumnTrait, DatabaseTransaction, EntityTrait, QueryFilter, QueryOrder};

use crate::entities::task::{Column as TaskColumn, Entity as TaskEntity};
use crate::entities::task_event::{Column as EventColumn, Entity as EventEntity};

/// Serial gate: an assignee works one task at a time.
/// Returns the blocking task when the assignee already holds an
/// `in_progress` task other than `exclude_id`.
pub async fn serial_gate_blocked(
    tx: &DatabaseTransaction,
    assignee: &str,
    exclude_id: i64,
) -> Result<Option<(i64, String)>, sea_orm::DbErr> {
    let row = TaskEntity::find()
        .filter(TaskColumn::Assignee.eq(assignee))
        .filter(TaskColumn::Status.eq("in_progress"))
        .filter(TaskColumn::Id.ne(exclude_id))
        .one(tx)
        .await?;
    Ok(row.map(|t| (t.id, t.title)))
}

/// Close gate: every review seat registered at dispatch must have
/// filed a receipt event (`kind=action`, `name=receipt`, actor = the
/// seat) in the current review cycle. Reopening a task starts a new
/// cycle — the `reopen` transition event is the invalidation marker —
/// so receipts older than the latest reopen do not count. Returns the
/// seats whose current-cycle receipts are missing.
pub async fn missing_receipts(
    tx: &DatabaseTransaction,
    task_id: i64,
    seats: &[String],
) -> Result<Vec<String>, sea_orm::DbErr> {
    let boundary = latest_reopen_event_id(tx, task_id).await?;
    let receipts = EventEntity::find()
        .filter(EventColumn::TaskId.eq(task_id))
        .filter(EventColumn::Kind.eq("action"))
        .filter(EventColumn::Name.eq("receipt"))
        .filter(EventColumn::Id.gt(boundary))
        .all(tx)
        .await?;
    let filed: std::collections::HashSet<&str> =
        receipts.iter().filter_map(|e| e.actor.as_deref()).collect();
    Ok(seats
        .iter()
        .filter(|s| !filed.contains(s.as_str()))
        .cloned()
        .collect())
}

/// Id of the latest `reopen` transition event for the task; 0 when the
/// task was never reopened, so every receipt counts in the first cycle.
async fn latest_reopen_event_id(
    tx: &DatabaseTransaction,
    task_id: i64,
) -> Result<i64, sea_orm::DbErr> {
    let event = EventEntity::find()
        .filter(EventColumn::TaskId.eq(task_id))
        .filter(EventColumn::Kind.eq("transition"))
        .filter(EventColumn::Name.eq("reopen"))
        .order_by_desc(EventColumn::Id)
        .one(tx)
        .await?;
    Ok(event.map(|e| e.id).unwrap_or(0))
}
