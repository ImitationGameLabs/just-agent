//! The task coordination domain: REST over the task ledger. The tagma
//! process is the SOLE writer of tasks.sqlite — these routes are the
//! only write path; CLI processes never touch the file, they go
//! through this API.
//!
//! Every write verb maps 1:1 to a `kallip task` subcommand; responses
//! reuse the store's export face (stable field names, ISO 8601 UTC) so
//! the CLI renders exactly what `task export --json` prints. Gate and
//! transition errors are 409s (the state refused), unknown ids are 404s,
//! malformed request input (association keys, dossier paths) is 400s, and
//! anything unexpected is a 500 with the store error embedded.

use axum::Json;
use axum::extract::{Path, Query, State};
use kallip_common::protocol::ApiError;
use kallip_task::{CheckpointSpec, ClosedReason, CreateSpec, TaskFilter, TaskStore};
use serde::Deserialize;

use crate::state::SharedState;

#[derive(Deserialize)]
pub(crate) struct CreateTaskBody {
    pub title: String,
    pub creator: String,
    #[serde(default)]
    pub assignee: Option<String>,
    #[serde(default)]
    pub seats: Vec<String>,
    #[serde(default)]
    pub dossier_path: Option<String>,
    #[serde(default)]
    pub inbox_id_start: Option<i64>,
    #[serde(default)]
    pub inbox_id_end: Option<i64>,
    #[serde(default)]
    pub room_id: Option<String>,
    #[serde(default)]
    pub room_seq_start: Option<i64>,
    #[serde(default)]
    pub room_seq_end: Option<i64>,
}

#[derive(Deserialize)]
pub(crate) struct StartBody {
    pub actor: String,
    #[serde(default)]
    pub force: bool,
}

#[derive(Deserialize)]
pub(crate) struct CheckpointBody {
    pub actor: String,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub receipt: bool,
    #[serde(default)]
    pub review: bool,
    #[serde(default)]
    pub waiting: Option<bool>,
}

#[derive(Deserialize)]
pub(crate) struct AnnotateBody {
    pub actor: String,
    pub note: String,
}

#[derive(Deserialize)]
pub(crate) struct GateReportBody {
    pub actor: String,
    pub note: String,
}

#[derive(Deserialize)]
pub(crate) struct DispatchBody {
    pub actor: String,
    #[serde(default)]
    pub seats: Option<Vec<String>>,
}

#[derive(Deserialize)]
pub(crate) struct ChainOpBody {
    pub actor: String,
    pub op: String,
    #[serde(default)]
    pub detail: Option<String>,
    #[serde(default)]
    pub force: bool,
}

#[derive(Deserialize)]
pub(crate) struct CloseBody {
    pub actor: String,
    pub reason: ClosedReason,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub force: bool,
}

#[derive(Deserialize)]
pub(crate) struct ReopenBody {
    pub actor: String,
    #[serde(default)]
    pub force: bool,
}

#[derive(Deserialize)]
pub(crate) struct ArchiveBody {
    pub actor: String,
    #[serde(default)]
    pub force: bool,
}

#[derive(Deserialize)]
pub(crate) struct ListQuery {
    #[serde(default)]
    pub status: Option<kallip_task::TaskStatus>,
    #[serde(default)]
    pub assignee: Option<String>,
    #[serde(default)]
    pub archived: bool,
}

/// The store lives behind a OnceLock installed at boot; a route seeing
/// None means the tagma booted without it, which is an operator-facing
/// configuration failure, not a caller error.
fn store(state: &SharedState) -> Result<&TaskStore, ApiError> {
    state
        .tasks
        .get()
        .map(|a| a.as_ref())
        .ok_or_else(|| ApiError::internal("task store not installed"))
}

fn blobs(state: &SharedState) -> Option<std::sync::Arc<dyn kallip_task::BlobStore>> {
    state.task_blobs.get().cloned()
}

/// The store's error taxonomy to the HTTP face: state refusals are
/// conflicts, unknown ids are misses, everything else is ours.
fn api_error(err: kallip_task::Error) -> ApiError {
    use kallip_task::Error as E;
    match &err {
        E::NotFound { id } => ApiError::not_found(format!("task {id} not found")),
        E::InvalidTransition { .. }
        | E::SerialGate { .. }
        | E::ReceiptGate { .. }
        | E::DispatchGate { .. }
        | E::GateReportGate { .. }
        | E::ArchiveGate { .. } => ApiError::conflict(err.to_string()),
        _ => ApiError::internal(err.to_string()),
    }
}

type TaskResult<T> = Result<Json<T>, ApiError>;

async fn create(
    State(state): State<SharedState>,
    Json(body): Json<CreateTaskBody>,
) -> TaskResult<kallip_task::TaskExport> {
    let spec = CreateSpec {
        title: body.title,
        creator: body.creator,
        assignee: body.assignee,
        seats: body.seats,
        dossier_path: body.dossier_path,
        inbox_id_start: body.inbox_id_start,
        inbox_id_end: body.inbox_id_end,
        room_id: body.room_id,
        room_seq_start: body.room_seq_start,
        room_seq_end: body.room_seq_end,
    };
    let task = store(&state)?.create(spec).await.map_err(api_error)?;
    let id = task.id;
    let export = store(&state)?.export(id).await.map_err(api_error)?;
    Ok(Json(export))
}

async fn start(
    State(state): State<SharedState>,
    Path(id): Path<i64>,
    Json(body): Json<StartBody>,
) -> TaskResult<kallip_task::TaskExport> {
    store(&state)?
        .start(id, &body.actor, body.force)
        .await
        .map_err(api_error)?;
    let export = store(&state)?.export(id).await.map_err(api_error)?;
    Ok(Json(export))
}

/// The list face is the compact store row (no trail); the export face is
/// the per-task detail. List first, then export what you need.
async fn list(
    State(state): State<SharedState>,
    Query(query): Query<ListQuery>,
) -> TaskResult<Vec<kallip_task::TaskExport>> {
    let filter = TaskFilter {
        status: query.status,
        assignee: query.assignee,
        archived: query.archived,
    };
    let rows = store(&state)?.list(filter).await.map_err(api_error)?;
    let all = store(&state)?.export_all().await.map_err(api_error)?;
    let ids: std::collections::HashSet<i64> = rows.iter().map(|t| t.id).collect();
    Ok(Json(
        all.into_iter().filter(|e| ids.contains(&e.id)).collect(),
    ))
}

async fn show(
    State(state): State<SharedState>,
    Path(id): Path<i64>,
) -> TaskResult<kallip_task::TaskExport> {
    let export = store(&state)?.export(id).await.map_err(api_error)?;
    Ok(Json(export))
}

async fn export_one(
    State(state): State<SharedState>,
    Path(id): Path<i64>,
) -> TaskResult<kallip_task::TaskExport> {
    let export = store(&state)?.export(id).await.map_err(api_error)?;
    Ok(Json(export))
}

async fn export_all(State(state): State<SharedState>) -> TaskResult<Vec<kallip_task::TaskExport>> {
    let all = store(&state)?.export_all().await.map_err(api_error)?;
    Ok(Json(all))
}

async fn checkpoint(
    State(state): State<SharedState>,
    Path(id): Path<i64>,
    Json(body): Json<CheckpointBody>,
) -> TaskResult<kallip_task::TaskExport> {
    let spec = CheckpointSpec {
        id,
        actor: body.actor,
        note: body.note,
        receipt: body.receipt,
        review: body.review,
        waiting: body.waiting,
    };
    store(&state)?.checkpoint(spec).await.map_err(api_error)?;
    let export = store(&state)?.export(id).await.map_err(api_error)?;
    Ok(Json(export))
}

async fn annotate(
    State(state): State<SharedState>,
    Path(id): Path<i64>,
    Json(body): Json<AnnotateBody>,
) -> TaskResult<kallip_task::TaskExport> {
    store(&state)?
        .annotate(id, &body.actor, body.note)
        .await
        .map_err(api_error)?;
    let export = store(&state)?.export(id).await.map_err(api_error)?;
    Ok(Json(export))
}

async fn gate_report(
    State(state): State<SharedState>,
    Path(id): Path<i64>,
    Json(body): Json<GateReportBody>,
) -> TaskResult<kallip_task::TaskExport> {
    store(&state)?
        .gate_report(id, &body.actor, body.note)
        .await
        .map_err(api_error)?;
    let export = store(&state)?.export(id).await.map_err(api_error)?;
    Ok(Json(export))
}

async fn dispatch(
    State(state): State<SharedState>,
    Path(id): Path<i64>,
    Json(body): Json<DispatchBody>,
) -> TaskResult<kallip_task::TaskExport> {
    store(&state)?
        .dispatch(id, &body.actor, body.seats)
        .await
        .map_err(api_error)?;
    let export = store(&state)?.export(id).await.map_err(api_error)?;
    Ok(Json(export))
}

async fn chain_op(
    State(state): State<SharedState>,
    Path(id): Path<i64>,
    Json(body): Json<ChainOpBody>,
) -> TaskResult<kallip_task::TaskExport> {
    store(&state)?
        .chain_op(id, &body.actor, &body.op, body.detail, body.force)
        .await
        .map_err(api_error)?;
    let export = store(&state)?.export(id).await.map_err(api_error)?;
    Ok(Json(export))
}

async fn close(
    State(state): State<SharedState>,
    Path(id): Path<i64>,
    Json(body): Json<CloseBody>,
) -> TaskResult<kallip_task::TaskExport> {
    store(&state)?
        .close(
            id,
            &body.actor,
            body.reason,
            body.summary,
            body.force,
            blobs(&state),
        )
        .await
        .map_err(api_error)?;
    let export = store(&state)?.export(id).await.map_err(api_error)?;
    Ok(Json(export))
}

async fn reopen(
    State(state): State<SharedState>,
    Path(id): Path<i64>,
    Json(body): Json<ReopenBody>,
) -> TaskResult<kallip_task::TaskExport> {
    store(&state)?
        .reopen(id, &body.actor, body.force)
        .await
        .map_err(api_error)?;
    let export = store(&state)?.export(id).await.map_err(api_error)?;
    Ok(Json(export))
}

async fn archive(
    State(state): State<SharedState>,
    Path(id): Path<i64>,
    Json(body): Json<ArchiveBody>,
) -> TaskResult<kallip_task::TaskExport> {
    store(&state)?
        .archive_task(id, &body.actor, body.force)
        .await
        .map_err(api_error)?;
    let export = store(&state)?.export(id).await.map_err(api_error)?;
    Ok(Json(export))
}

/// The task-domain router: mounted at /tasks by the root router.
pub(crate) fn router() -> axum::Router<SharedState> {
    axum::Router::new()
        .route("/", axum::routing::post(create).get(list))
        .route("/export", axum::routing::get(export_all))
        .route("/{id}", axum::routing::get(show))
        .route("/{id}/export", axum::routing::get(export_one))
        .route("/{id}/start", axum::routing::post(start))
        .route("/{id}/checkpoint", axum::routing::post(checkpoint))
        .route("/{id}/annotate", axum::routing::post(annotate))
        .route("/{id}/gate-report", axum::routing::post(gate_report))
        .route("/{id}/dispatch", axum::routing::post(dispatch))
        .route("/{id}/chain-op", axum::routing::post(chain_op))
        .route("/{id}/close", axum::routing::post(close))
        .route("/{id}/reopen", axum::routing::post(reopen))
        .route("/{id}/archive", axum::routing::post(archive))
}
