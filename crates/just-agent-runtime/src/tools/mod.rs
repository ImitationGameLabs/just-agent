use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use just_llm_client::ToolDispatcher;
use just_llm_client::types::chat::{FunctionDefinition, ToolDefinition, ToolType};
use serde_json::json;
use shell::{PtyBuilder, shell_tool_set};
use tokio::sync::Mutex;

use crate::context::{AgenticContext, ContextStore};
pub mod context;
pub mod shell;
pub mod skill;

pub use skill::{ensure_meta_skill, load_skill};

/// Builds the tool registry exposed by `just-agent`.
///
/// Spawns bash via [`PtyBuilder`], preserving full shell session state.
/// The shell's working directory is the process current directory (set by
/// the caller via `std::env::set_current_dir`).
///
/// Context tools share the same `ContextStore` as the main loop.
pub async fn build_tool_dispatch(
    ctx: Arc<Mutex<ContextStore>>,
    env: HashMap<String, String>,
) -> Result<ToolDispatcher> {
    let backend = PtyBuilder::new("main").envs(env).build().await?;
    let backend = Arc::new(Mutex::new(backend));

    let mut dispatch = ToolDispatcher::new();
    dispatch.add_tools(shell_tool_set(backend))?;
    let ctx_dyn: Arc<Mutex<dyn AgenticContext>> = ctx;
    dispatch.add_tools(context::context_tool_set(ctx_dyn.clone()))?;
    dispatch.add_tools(skill::skill_tool_set(ctx_dyn))?;

    Ok(dispatch)
}

// ---------------------------------------------------------------------------
// Deferred action meta-tool definitions (handled by executor, not dispatcher)
// ---------------------------------------------------------------------------

pub fn deferred_action_list_definition() -> ToolDefinition {
    ToolDefinition {
        kind: ToolType::Function,
        function: FunctionDefinition {
            name: "deferred_action_list".into(),
            description: Some(
                "List deferred tool actions awaiting or having received approval. \
                 Filter by status: pending, committed, approved, denied, redeemed, cancelled. \
                 Returns action details including id needed for commit/redeem/cancel."
                    .into(),
            ),
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "status": {
                        "type": "string",
                        "enum": ["pending", "committed", "approved", "denied", "redeemed", "cancelled", "all"],
                        "description": "Filter by status. Omit to list all."
                    }
                }
            })),
            strict: None,
        },
    }
}

pub fn deferred_action_commit_definition() -> ToolDefinition {
    ToolDefinition {
        kind: ToolType::Function,
        function: FunctionDefinition {
            name: "deferred_action_commit".into(),
            description: Some(
                "Submit a deferred action for approval with your justification for \
                 why this tool call is necessary. After committing, the action becomes \
                 visible to an approver. Only works on actions with 'pending' status."
                    .into(),
            ),
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "The id of the deferred action to commit."
                    },
                    "reason": {
                        "type": "string",
                        "description": "Your justification for why this tool call is necessary."
                    }
                },
                "required": ["id", "reason"]
            })),
            strict: None,
        },
    }
}

pub fn deferred_action_redeem_definition() -> ToolDefinition {
    ToolDefinition {
        kind: ToolType::Function,
        function: FunctionDefinition {
            name: "deferred_action_redeem".into(),
            description: Some(
                "Execute a previously deferred tool action that has been approved. \
                 The stored tool call runs and returns its result. \
                 Only works on actions with 'approved' status."
                    .into(),
            ),
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "The id of the deferred action to redeem."
                    }
                },
                "required": ["id"]
            })),
            strict: None,
        },
    }
}

pub fn deferred_action_cancel_definition() -> ToolDefinition {
    ToolDefinition {
        kind: ToolType::Function,
        function: FunctionDefinition {
            name: "deferred_action_cancel".into(),
            description: Some(
                "Cancel a deferred action that is no longer needed. \
                 Works on pending, committed, approved, and denied actions."
                    .into(),
            ),
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "The id of the deferred action to cancel."
                    }
                },
                "required": ["id"]
            })),
            strict: None,
        },
    }
}
