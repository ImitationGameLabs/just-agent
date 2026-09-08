//! The `kallip task` family: thin rendering over the kallip-task store.
//! Process-local — no tagma daemon connection. Storage hangs off the tagma
//! data dir: `tasks.sqlite` plus the `task-blobs/` content-addressed root
//! for closed archives. Every write verb resolves the acting agent from
//! `KALLIP_ID` (or --actor) and passes it down as the event `actor`.

use anyhow::{Result, anyhow};
use kallip_blob_store::LocalBackend;
use kallip_task::store::{CheckpointSpec, CreateSpec, TaskExport, TaskFilter};
use kallip_task::{ClosedReason, TaskStatus, TaskStore};

use crate::args::{TaskChainOpType, TaskCloseReason, TaskCommand, TaskStartArgs};

pub async fn run_task(cmd: &TaskCommand) -> Result<()> {
    let data_root = kallip_runtime::persistence::data_dir_root()?;
    let store = TaskStore::open(&data_root.join("tasks.sqlite")).await?;
    let blobs = LocalBackend::arc(data_root.join("task-blobs"));

    match cmd {
        TaskCommand::Start(args) => {
            let actor = task_actor(args.actor.as_deref())?;
            let task = match args.id {
                Some(id) => {
                    if has_dispatch_meta(args) {
                        return Err(anyhow!(
                            "start <id> picks an existing task up; dispatch \
                             metadata (--title and friends) registers a new one"
                        ));
                    }
                    store.start(id, &actor, args.force).await?
                }
                None => {
                    let title = args.title.as_deref().ok_or_else(|| {
                        anyhow!("give a task id to pick up, or --title to register a new task")
                    })?;
                    store
                        .create(CreateSpec {
                            title: title.to_string(),
                            creator: args.creator.clone().unwrap_or_else(|| actor.clone()),
                            assignee: args.assignee.clone(),
                            seats: args.seats.clone(),
                            dossier_path: args.dossier.as_ref().map(|p| p.display().to_string()),
                            inbox_id_start: args.inbox_start,
                            inbox_id_end: args.inbox_end,
                            room_id: args.room.clone(),
                            room_seq_start: args.room_seq_start,
                            room_seq_end: args.room_seq_end,
                        })
                        .await?
                }
            };
            print_state_line(&task);
        }
        TaskCommand::Checkpoint(args) => {
            let actor = task_actor(args.actor.as_deref())?;
            let waiting = tri_flag(args.waiting, args.no_waiting)?;
            let task = store
                .checkpoint(CheckpointSpec {
                    id: args.id,
                    actor,
                    note: args.note.clone(),
                    receipt: args.receipt,
                    review: args.review,
                    waiting,
                })
                .await?;
            print_state_line(&task);
        }
        TaskCommand::Close(args) => {
            let actor = task_actor(args.actor.as_deref())?;
            let task = store
                .close(
                    args.id,
                    &actor,
                    close_reason(args.reason),
                    args.summary.clone(),
                    args.force,
                    Some(blobs),
                )
                .await?;
            print_state_line(&task);
            if let Some(hash) = &task.archive_hash {
                println!("archive: {hash}");
            }
            if let Some(summary) = &task.close_summary {
                println!("summary: {summary}");
            }
        }
        TaskCommand::Reopen(args) => {
            let actor = task_actor(args.actor.as_deref())?;
            let task = store.reopen(args.id, &actor, args.force).await?;
            print_state_line(&task);
        }

        TaskCommand::Annotate(args) => {
            let actor = task_actor(args.actor.as_deref())?;
            let task = store.annotate(args.id, &actor, args.note.clone()).await?;
            print_state_line(&task);
        }
        TaskCommand::Dispatch(args) => {
            let actor = task_actor(args.actor.as_deref())?;
            // Blank seat entries are dropped: a blank seat name would
            // ghost the close gate forever. `--seats ""` therefore
            // registers an explicit empty roster; omitting --seats
            // re-affirms the registered seats.
            let seats = args.seats.clone().map(|list| {
                list.into_iter()
                    .filter(|s| !s.trim().is_empty())
                    .collect::<Vec<String>>()
            });
            let task = store.dispatch(args.id, &actor, seats).await?;
            print_state_line(&task);
        }
        TaskCommand::GateReport(args) => {
            let actor = task_actor(args.actor.as_deref())?;
            let task = store
                .gate_report(args.id, &actor, args.note.clone())
                .await?;
            print_state_line(&task);
        }
        TaskCommand::ChainOp(args) => {
            let actor = task_actor(args.actor.as_deref())?;
            let task = store
                .chain_op(
                    args.id,
                    &actor,
                    chain_op_name(args.op),
                    args.detail.clone(),
                    args.force,
                )
                .await?;
            print_state_line(&task);
        }
        TaskCommand::Archive(args) => {
            let actor = task_actor(args.actor.as_deref())?;
            let task = store.archive_task(args.id, &actor, args.force).await?;
            print_state_line(&task);
        }
        TaskCommand::List(args) => {
            let status = args
                .status
                .as_deref()
                .map(|s| {
                    TaskStatus::parse(s).ok_or_else(|| {
                        anyhow!("unknown status '{s}' (queued|in_progress|review|closed)")
                    })
                })
                .transpose()?;
            let tasks = store
                .list(TaskFilter {
                    status,
                    archived: args.archived,
                    assignee: args.assignee.clone(),
                })
                .await?;
            if tasks.is_empty() {
                println!("(no tasks)");
            } else {
                for t in &tasks {
                    println!(
                        "{:>4}  {:<11} {:<16} {}",
                        t.id,
                        t.status,
                        t.assignee.as_deref().unwrap_or("-"),
                        t.title
                    );
                }
                println!("(showing {})", tasks.len());
            }
        }
        TaskCommand::Show(args) => {
            let export = store.export(args.id).await?;
            print_show(&export);
        }
        TaskCommand::Export(args) => {
            let mut exports = Vec::new();
            if args.all {
                exports = store.export_all().await?;
            } else {
                let id = args.id.ok_or_else(|| anyhow!("give a task id, or --all"))?;
                exports.push(store.export(id).await?);
            }
            if args.json {
                println!("{}", serde_json::to_string_pretty(&exports)?);
            } else {
                for e in &exports {
                    print_show(e);
                    println!();
                }
            }
        }
        TaskCommand::Extract(args) => {
            let (task, _) = store.get(args.id).await?;
            let blob = TaskStore::archive_blob_id(&task)?
                .ok_or_else(|| anyhow!("task {} has no closed archive", args.id))?;
            kallip_task::archive::extract(blobs.as_ref(), &blob, &args.to).await?;
            println!(
                "extracted task {} archive to {}",
                args.id,
                args.to.display()
            );
        }
    }
    Ok(())
}

fn print_state_line(task: &kallip_task::Task) {
    println!("task {} {} '{}'", task.id, task.status, task.title);
}

fn print_show(e: &TaskExport) {
    println!("task {}: '{}'", e.id, e.title);
    println!(
        "status: {}  assignee: {}  creator: {}",
        e.status,
        e.assignee.as_deref().unwrap_or("-"),
        e.creator.as_deref().unwrap_or("-")
    );
    if !e.seats.is_empty() {
        println!("seats: {}", e.seats.join(", "));
    }
    if e.waiting {
        println!(
            "waiting: yes (since {})",
            e.waiting_since.as_deref().unwrap_or("?")
        );
    }
    if let Some(a) = &e.association {
        let mut parts = Vec::new();
        if let (Some(from), Some(to)) = (a.inbox_id_start, a.inbox_id_end) {
            parts.push(format!("inbox {from}..{to}"));
        }
        if let Some(room) = &a.room_id {
            match (a.room_seq_start, a.room_seq_end) {
                (Some(from), Some(to)) => parts.push(format!("room {room} seq {from}..{to}")),
                _ => parts.push(format!("room {room}")),
            }
        }
        if !parts.is_empty() {
            println!("association: {}", parts.join("; "));
        }
    }
    if let Some(path) = &e.dossier_path {
        println!("dossier: {path} (live)");
    }
    if let Some(hash) = &e.archive_hash {
        println!("archive: {hash} (closed)");
    }
    if let Some(reason) = &e.closed_reason {
        match &e.close_summary {
            Some(summary) => println!("closed: {reason} — {summary}"),
            None => println!("closed: {reason}"),
        }
    }
    println!("events:");
    for ev in &e.events {
        let scope = match (&ev.from_status, &ev.to_status) {
            (Some(from), Some(to)) => format!(" ({from} -> {to})"),
            _ => String::new(),
        };
        println!(
            "  {} {} {}{} by {}",
            ev.created_at.as_deref().unwrap_or("?"),
            ev.kind,
            ev.name,
            scope,
            ev.actor.as_deref().unwrap_or("-")
        );
    }
}

fn task_actor(flag: Option<&str>) -> Result<String> {
    flag.map(str::to_string)
        .or_else(|| std::env::var("KALLIP_ID").ok())
        .ok_or_else(|| anyhow!("KALLIP_ID not set and --actor not given"))
}

fn tri_flag(set: bool, clear: bool) -> Result<Option<bool>> {
    match (set, clear) {
        (true, true) => Err(anyhow!("--waiting and --no-waiting are mutually exclusive")),
        (true, false) => Ok(Some(true)),
        (false, true) => Ok(Some(false)),
        (false, false) => Ok(None),
    }
}

fn close_reason(reason: TaskCloseReason) -> ClosedReason {
    match reason {
        TaskCloseReason::Completed => ClosedReason::Completed,
        TaskCloseReason::NotPlanned => ClosedReason::NotPlanned,
        TaskCloseReason::Duplicate => ClosedReason::Duplicate,
    }
}

fn chain_op_name(op: TaskChainOpType) -> &'static str {
    match op {
        TaskChainOpType::Commit => "commit",
        TaskChainOpType::Amend => "amend",
        TaskChainOpType::Rebase => "rebase",
        TaskChainOpType::Reset => "reset",
    }
}

fn has_dispatch_meta(args: &TaskStartArgs) -> bool {
    args.title.is_some()
        || args.creator.is_some()
        || args.assignee.is_some()
        || !args.seats.is_empty()
        || args.dossier.is_some()
        || args.inbox_start.is_some()
        || args.inbox_end.is_some()
        || args.room.is_some()
        || args.room_seq_start.is_some()
        || args.room_seq_end.is_some()
}
