//! just-agent-run: one-shot agent runner for scripting and benchmarking.
//!
//! Non-interactive CLI that creates an agent via the daemon, streams progress
//! to stderr, prints the final result to stdout, and exits with a semantic
//! exit code. Designed for scripted and automated workflows where the caller
//! needs machine-readable output and exit-status-driven control flow.

use std::process::ExitCode;

use anyhow::Result;
use clap::Parser;
use futures_util::StreamExt;
use just_agent_client::DaemonClient;
use just_agent_common::agentid::AgentId;
use just_agent_common::protocol::{CreateAgentRequest, MaxToolRounds, SseEvent};

#[derive(Parser)]
#[command(
    name = "just-agent-run",
    about = "Create an agent, run it to completion, and print the result"
)]
struct Cli {
    /// The prompt to send to the agent.
    prompt: String,
    /// Working directory for the agent.
    #[arg(long)]
    workspace_root: Option<String>,
    /// Maximum tool-call rounds for this agent run.
    /// Overrides the daemon default (unlimited unless JUST_AGENT_MAX_TOOL_ROUNDS is set).
    #[arg(long)]
    max_rounds: Option<usize>,
}

/// Semantic exit codes for `just-agent-run`.
///
/// Mapped to process exit codes via `#[repr(u8)]`:
/// 0 = success, 1 = error, 2 = max rounds exceeded,
/// 3 = cancelled, 4 = token budget exceeded.
#[derive(Clone, Copy)]
#[repr(u8)]
enum RunExit {
    Success = 0,
    Error = 1,
    MaxRounds = 2,
    Cancelled = 3,
    BudgetExceeded = 4,
}

impl From<RunExit> for ExitCode {
    fn from(code: RunExit) -> Self {
        ExitCode::from(code as u8)
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e:#}");
            RunExit::Error.into()
        }
    }
}

async fn run() -> Result<ExitCode> {
    let cli = Cli::parse();
    let client = DaemonClient::from_env()?;

    let id = client
        .spawn(CreateAgentRequest {
            workspace_root: cli.workspace_root,
            skills: vec![],
            prompt: Some(cli.prompt),
            created_by: std::env::var("JUST_AGENT_ID").ok().map(AgentId::from),
            max_tool_rounds: cli.max_rounds.map(MaxToolRounds::Limited),
        })
        .await?;

    let exit = consume_stream(&client, &id).await;

    // Clean up the agent regardless of outcome.
    if let Err(e) = client.stop_agent(&id).await {
        eprintln!("warning: failed to stop agent {id}: {e}");
    }

    Ok(exit.into())
}

/// Subscribe to the agent's SSE stream and print events until a terminal
/// event arrives.
///
/// Returns the exit status. Defaults to [`RunExit::Error`] if the stream
/// closes without a terminal event (daemon crash, network drop).
async fn consume_stream(client: &DaemonClient, id: &AgentId) -> RunExit {
    let mut stream = match client.event_stream(id).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("failed to subscribe to agent events: {e}");
            return RunExit::Error;
        }
    };

    // Default to error — only the Finished arm sets success.
    // If the stream closes without a terminal event, we correctly report failure.
    let mut exit = RunExit::Error;

    while let Some(result) = stream.next().await {
        let event = match result {
            Ok(e) => e,
            Err(e) => {
                eprintln!("SSE error: {e}");
                return RunExit::Error;
            }
        };
        match event {
            SseEvent::AssistantContentDelta { delta } => {
                eprint!("{delta}");
            }
            SseEvent::ReasoningDelta { delta } => {
                eprint!("[reasoning] {delta}");
            }
            SseEvent::ToolCall { name, .. } => {
                eprintln!("[tool] {name}");
            }
            SseEvent::ToolResult { result } => {
                eprintln!("[tool-result] {result}");
            }
            SseEvent::Retrying {
                attempt,
                max_attempts,
                error,
                delay_secs,
            } => {
                eprintln!("[retry {attempt}/{max_attempts}] {error} (waiting {delay_secs:.1}s)");
            }
            SseEvent::Finished { content } => {
                print!("{content}");
                exit = RunExit::Success;
                break;
            }
            SseEvent::Error { message } => {
                eprintln!("{message}");
                return RunExit::Error;
            }
            SseEvent::MaxRoundsExceeded => {
                eprintln!("max rounds exceeded");
                return RunExit::MaxRounds;
            }
            SseEvent::Cancelled => {
                eprintln!("cancelled");
                return RunExit::Cancelled;
            }
            SseEvent::TokenBudgetExceeded { consumed, budget } => {
                eprintln!("token budget exceeded (consumed: {consumed}, budget: {budget})");
                return RunExit::BudgetExceeded;
            }
            // Suppress noise for one-shot mode.
            SseEvent::Busy
            | SseEvent::Status { .. }
            | SseEvent::ApprovalUpdated { .. }
            | SseEvent::AssistantContent { .. }
            | SseEvent::Reasoning { .. } => {}
        }
    }

    exit
}
