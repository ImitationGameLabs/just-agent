//! Slash command definitions and user input types.
//!
//! Shared between the daemon (produces `UserInput`), runtime (consumes it),
//! and the TUI (parses and dispatches commands).

/// Input from the TUI, sent through the prompt channel.
pub enum UserInput {
    /// A normal chat message to send to the LLM.
    Prompt(String),
    /// A slash command to execute.
    Command(SlashCommand),
}

/// A budget operation parsed from `/budget +N/-N/=N`.
#[derive(Debug)]
pub enum BudgetOp {
    /// Adjust total budget by signed delta (`+100M` or `-50M`).
    Adjust(i64),
    /// Set remaining budget to this value (`=5M` → new total = consumed + 5M).
    Set(u64),
}

/// A parsed slash command.
#[derive(Debug)]
pub enum SlashCommand {
    Help,
    Quit,
    Clear,
    Status,
    Approvals,
    /// `/budget` with no args → status query; with args → adjust or set.
    Budget {
        op: Option<BudgetOp>,
    },
}
