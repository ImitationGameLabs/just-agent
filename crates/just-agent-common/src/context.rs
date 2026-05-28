//! Context usage snapshot type.

/// Snapshot of current context layer breakdown and last known token usage.
///
/// `last_prompt_tokens` comes from the provider's response `usage` field —
/// the most accurate token count available. Layer breakdowns use heuristic
/// estimates for informational purposes.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ContextUsage {
    /// Per-item breakdown: (label, estimated_tokens).
    pub pinned_items: Vec<(String, usize)>,
    /// Number of stored conversation turns.
    pub turn_count: usize,
    /// Estimated tokens across all turns.
    pub turn_tokens: usize,
    /// Exact prompt token count from the last provider response, if any.
    pub last_prompt_tokens: Option<u32>,
}

impl ContextUsage {
    pub fn format_summary(&self) -> String {
        let pinned_tokens: usize = self.pinned_items.iter().map(|(_, t)| *t).sum();
        format!(
            "turns: {} ({} est tokens), pinned: {} ({} tokens), last prompt: {}",
            self.turn_count,
            self.turn_tokens,
            self.pinned_items.len(),
            pinned_tokens,
            self.last_prompt_tokens
                .map(|t| t.to_string())
                .unwrap_or_else(|| "n/a".into()),
        )
    }
}
