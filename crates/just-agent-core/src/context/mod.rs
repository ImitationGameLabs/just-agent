//! Agentic context management module.
//!
//! - [`ContextStore`] — single source of truth for all context data
//! - [`compose_context`] — assembles layers into `Vec<ChatMessage>`
//! - [`ContextSummarizer`] — LLM-powered summarization of old turns
//!
//! Layers are filled in priority order: pinned → turns.
//! Budget checking uses accurate token estimation via ChatClient.

mod compose;
mod store;
mod summarize;
mod turn;

pub use compose::compose_context;
pub use store::{AgenticContext, ContextStore, ContextUsage};
pub use summarize::{ContextSummarizer, Summary};
