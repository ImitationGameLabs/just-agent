//! kallipctl: operator-side management CLI for the kallip local daemon.
//!
//! Four verbs over the daemon's UDS protocol; the socket's 0600 mode is the
//! auth. Deliberately NOT part of the `kallip` command family: `kallip` is
//! the in-instance runtime, `kallipctl` manages instances from outside
//! (separate installation surfaces).

use anyhow::{Context as _, Result};
use clap::{Parser, Subcommand};
use kallip_daemon_client::DaemonClient;
use kallip_daemon_common::wire::{ErrorCode, OkPayload, RequestBody, Response, ResponseBody};

#[derive(Parser)]
#[command(
    name = "kallipctl",
    about = "Manage local kallip instances via the kallip daemon",
    version
)]
struct Cli {
    /// Daemon control socket (default: $KALLIP_DAEMON_SOCKET, else
    /// $KALLIP_STATE_DIR/control.sock, else the daemon default).
    #[arg(long, global = true)]
    socket: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Launch a new instance under a slug.
    Spawn {
        /// Instance slug ([a-z0-9][a-z0-9-]*).
        slug: String,
        /// Absolute path of the instance workspace.
        workspace: String,
        /// Extra env for the instance, KEY=VALUE (repeatable); only
        /// KALLIP_* keys and RUST_LOG are accepted by the daemon.
        #[arg(short = 'e', long = "env")]
        env: Vec<String>,
    },
    /// Terminate an instance (TERM, grace, KILL).
    Stop { slug: String },
    /// List managed instances.
    List,
    /// Daemon health, or one instance's health by slug.
    Health { slug: Option<String> },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let socket = cli
        .socket
        .clone()
        .or_else(|| std::env::var("KALLIP_DAEMON_SOCKET").ok())
        .unwrap_or_else(|| {
            // Mirror the daemon's default resolution without importing it.
            std::env::var_os("KALLIP_STATE_DIR")
                .map(|d| format!("{}/control.sock", d.to_string_lossy()))
                .unwrap_or_else(|| {
                    format!(
                        "{}/kallip-daemon/control.sock",
                        std::env::var_os("XDG_STATE_HOME")
                            .map(|h| h.to_string_lossy().into_owned())
                            .unwrap_or_else(|| format!(
                                "{}/.local/state",
                                std::env::var_os("HOME")
                                    .map(|h| h.to_string_lossy().into_owned())
                                    .unwrap_or_default()
                            ))
                    )
                })
        });
    let client = DaemonClient::new(socket);

    let body = match cli.command {
        Command::Spawn {
            slug,
            workspace,
            env,
        } => RequestBody::Spawn {
            slug,
            workspace,
            env,
        },
        Command::Stop { slug } => RequestBody::Stop { slug },
        Command::List => RequestBody::List,
        Command::Health { slug } => RequestBody::Health { slug },
    };
    let response = client
        .call(body)
        .await
        .context("talking to the kallip daemon")?;
    print(response, &client).await
}

async fn print(response: Response, client: &DaemonClient) -> Result<()> {
    match response.body {
        ResponseBody::Ok { payload } => {
            match payload {
                OkPayload::Spawn { slug, pid, port } => {
                    println!("spawned {slug} (pid {pid}, port {port})");
                }
                OkPayload::Stop { slug } => {
                    println!("stopped {slug}");
                }
                OkPayload::List { instances } => {
                    if instances.is_empty() {
                        println!("no instances");
                        return Ok(());
                    }
                    // One labeled block per instance: every field is a full
                    // greppable line, so agents and scripts read records
                    // without parsing column alignment.
                    for (n, i) in instances.iter().enumerate() {
                        if n > 0 {
                            println!();
                        }
                        println!("{}", i.slug);
                        // The list wire carries only a live/stopped
                        // boolean; one per-slug health probe recovers the
                        // daemon's richer state (stale pid vs no pid file).
                        let (state, detail) = if i.running {
                            ("running", None)
                        } else {
                            match client
                                .call(RequestBody::Health {
                                    slug: Some(i.slug.clone()),
                                })
                                .await
                            {
                                Ok(Response {
                                    body:
                                        ResponseBody::Ok {
                                            payload: OkPayload::Health { report },
                                        },
                                    ..
                                }) => match report.detail.as_deref() {
                                    // "no pid file" is the scan module's
                                    // wording — pinned by its dead-pid tests;
                                    // any other detail means the pid went stale.
                                    Some("no pid file") => ("stopped", None),
                                    _ => ("dead", report.detail),
                                },
                                // A probe can only race the list scan (the
                                // instance vanished in between); degrade to
                                // the boolean's word rather than failing.
                                _ => ("stopped", None),
                            }
                        };
                        println!("  state: {state}");
                        if let Some(d) = detail {
                            println!("    detail: {d}");
                        }
                        println!("  workspace: {}", i.workspace);
                        println!(
                            "  owner-uid: {}",
                            i.owner.map(|u| u.to_string()).unwrap_or_else(|| "-".into())
                        );
                        println!("  instance-id: {}", i.instance_id);
                    }
                }
                OkPayload::Health { report } => match (&report.slug, report.running) {
                    (None, _) => println!("daemon: healthy"),
                    (Some(slug), true) => println!("{slug}: running"),
                    (Some(slug), false) => {
                        println!(
                            "{slug}: not running ({})",
                            report.detail.unwrap_or_default()
                        );
                    }
                },
            }
            Ok(())
        }
        ResponseBody::Err { code, message } => {
            // Errors exit non-zero with a human line; stable codes are the
            // machine interface for scripting.
            let prefix = match code {
                ErrorCode::SlugTaken => "slug already exists",
                ErrorCode::WorkspaceOverlap => "workspace overlaps an existing instance",
                ErrorCode::InvalidSpawnInput => "invalid spawn input",
                ErrorCode::SpawnTimeout => "spawn timed out (rolled back)",
                ErrorCode::NotFound => "no such instance",
                ErrorCode::NotRunning => "not running",
                ErrorCode::BadRequest => "bad request",
                ErrorCode::Internal => "internal error",
            };
            anyhow::bail!("{prefix}: {message}");
        }
    }
}
