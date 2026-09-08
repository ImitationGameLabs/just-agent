//! The event-boundary hard gates, enforced inside the transaction that
//! performs the gated action.
//!
//! Serialization point: SQLite WAL. Every write transaction takes the
//! write lock with its first statement (see `store::take_write_lock`),
//! so the gate check reads under the lock: two concurrent gated
//! transitions queue on `busy_timeout` instead of racing — the loser
//! waits for the winner to commit, then re-checks the gate against the
//! fresh state.
//!
//! Close-gate receipts are cycle-scoped: a review cycle starts at create
//! or at the latest `reopen` transition, and a `dispatch` action re-bases
//! the receipt boundary — only receipts filed after the later of the two
//! count.

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
    let boundary = receipt_boundary(tx, task_id).await?;
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

/// Receipt boundary: the later of the latest `reopen` transition (the
/// cycle start) and the latest `dispatch` action (the roster
/// re-registration). 0 when neither exists, so first-cycle receipts all
/// count.
async fn receipt_boundary(tx: &DatabaseTransaction, task_id: i64) -> Result<i64, sea_orm::DbErr> {
    Ok(latest_reopen_event_id(tx, task_id)
        .await?
        .max(latest_action_event_id(tx, task_id, "dispatch").await?))
}

/// The latest `dispatch` action event, if any: its payload carries the
/// seat roster the close gate counts receipts against.
pub async fn latest_dispatch(
    tx: &DatabaseTransaction,
    task_id: i64,
) -> Result<Option<crate::entities::task_event::Model>, sea_orm::DbErr> {
    EventEntity::find()
        .filter(EventColumn::TaskId.eq(task_id))
        .filter(EventColumn::Kind.eq("action"))
        .filter(EventColumn::Name.eq("dispatch"))
        .order_by_desc(EventColumn::Id)
        .one(tx)
        .await
}

/// Dispatch gate: a dispatch must exist in the current review cycle
/// (after the latest reopen, if any) for `close` to pass without force.
pub async fn dispatch_in_current_cycle(
    tx: &DatabaseTransaction,
    task_id: i64,
) -> Result<bool, sea_orm::DbErr> {
    Ok(latest_action_event_id(tx, task_id, "dispatch").await?
        > latest_reopen_event_id(tx, task_id).await?)
}

/// Gate-report gate: a gate report must exist after the last recorded
/// chain operation (after every chain op, when none is recorded yet).
pub async fn gate_report_current(
    tx: &DatabaseTransaction,
    task_id: i64,
) -> Result<bool, sea_orm::DbErr> {
    Ok(latest_action_event_id(tx, task_id, "gate_report").await?
        > latest_action_event_id(tx, task_id, "chain_op").await?)
}

/// Id of the latest `action` event with the given name; 0 when none.
async fn latest_action_event_id(
    tx: &DatabaseTransaction,
    task_id: i64,
    name: &str,
) -> Result<i64, sea_orm::DbErr> {
    let event = EventEntity::find()
        .filter(EventColumn::TaskId.eq(task_id))
        .filter(EventColumn::Kind.eq("action"))
        .filter(EventColumn::Name.eq(name))
        .order_by_desc(EventColumn::Id)
        .one(tx)
        .await?;
    Ok(event.map(|e| e.id).unwrap_or(0))
}
