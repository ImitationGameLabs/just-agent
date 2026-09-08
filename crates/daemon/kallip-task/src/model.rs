//! The coarse state machine and the vocabulary the rest of the crate speaks.
//!
//! States are deliberately coarse (the K8s warning: a small state machine,
//! details live in the event trail). `waiting` is NOT a state — it is a
//! timing marker on the tasks row (taskwarrior precedent: an activity marker
//! is not a state transition). The terminal state is two-level (GitHub CLI
//! precedent): `closed` carries a reason.

use serde::{Deserialize, Serialize};

/// The four coarse states. Serialized lowercase on the wire and in SQLite.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskStatus {
    Queued,
    InProgress,
    Review,
    Closed,
}

impl TaskStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            TaskStatus::Queued => "queued",
            TaskStatus::InProgress => "in_progress",
            TaskStatus::Review => "review",
            TaskStatus::Closed => "closed",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "queued" => Some(TaskStatus::Queued),
            "in_progress" => Some(TaskStatus::InProgress),
            "review" => Some(TaskStatus::Review),
            "closed" => Some(TaskStatus::Closed),
            _ => None,
        }
    }
}

/// Why a task was closed. An attribute of `closed`, not a state of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClosedReason {
    Completed,
    NotPlanned,
    Duplicate,
}

impl ClosedReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            ClosedReason::Completed => "completed",
            ClosedReason::NotPlanned => "not_planned",
            ClosedReason::Duplicate => "duplicate",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "completed" => Some(ClosedReason::Completed),
            "not_planned" => Some(ClosedReason::NotPlanned),
            "duplicate" => Some(ClosedReason::Duplicate),
            _ => None,
        }
    }
}

/// Every legal transition, named by the verb that drives it. `checkpoint` is
/// deliberately absent: checkpoint/receipt/waiting are `kind=action` events
/// that never move the machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transition {
    /// queued -> in_progress (`task start <id>`; the serial gate lives here).
    Start,
    /// in_progress -> review (`task checkpoint --review`).
    Review,
    /// in_progress|review -> closed (`task close`; the receipt gate lives here).
    Close,
    /// closed -> in_progress (`task reopen`).
    Reopen,
}

impl Transition {
    pub fn from_str(&self) -> &'static str {
        match self {
            Transition::Start => TaskStatus::Queued.as_str(),
            Transition::Review => TaskStatus::InProgress.as_str(),
            Transition::Close => "", // in_progress or review; see `legal_from`
            Transition::Reopen => TaskStatus::Closed.as_str(),
        }
    }

    pub fn to_str(&self) -> &'static str {
        match self {
            Transition::Start => TaskStatus::InProgress.as_str(),
            Transition::Review => TaskStatus::Review.as_str(),
            Transition::Close => TaskStatus::Closed.as_str(),
            Transition::Reopen => TaskStatus::InProgress.as_str(),
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Transition::Start => "start",
            Transition::Review => "review",
            Transition::Close => "close",
            Transition::Reopen => "reopen",
        }
    }

    /// The states this transition may depart from.
    pub fn legal_from(&self) -> &'static [TaskStatus] {
        match self {
            Transition::Start => &[TaskStatus::Queued],
            Transition::Review => &[TaskStatus::InProgress],
            Transition::Close => &[TaskStatus::InProgress, TaskStatus::Review],
            Transition::Reopen => &[TaskStatus::Closed],
        }
    }

    pub fn legal_from_str(&self) -> String {
        self.legal_from()
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join("|")
    }
}

/// The two event classes. Transitions move the machine; actions never do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    Transition,
    Action,
}

impl EventKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            EventKind::Transition => "transition",
            EventKind::Action => "action",
        }
    }
}

/// Formats unix seconds as ISO 8601 UTC (`YYYY-MM-DDTHH:MM:SSZ`), the
/// stable machine-face representation for export JSON.
/// Hand-rolled because the workspace `time` build does not enable the
/// `formatting` feature.
pub fn iso8601_utc(secs: i64) -> Result<String, time::error::ComponentRange> {
    let dt = time::OffsetDateTime::from_unix_timestamp(secs)?;
    let (year, month, day) = dt.to_calendar_date();
    let (hh, mm, ss) = dt.time().as_hms();
    Ok(format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year,
        u8::from(month),
        day,
        hh,
        mm,
        ss
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_round_trips() {
        for raw in ["queued", "in_progress", "review", "closed"] {
            let parsed = TaskStatus::parse(raw).expect("parse");
            assert_eq!(parsed.as_str(), raw);
        }
        assert_eq!(TaskStatus::parse("active"), None);
    }

    #[test]
    fn closed_reason_round_trips() {
        for raw in ["completed", "not_planned", "duplicate"] {
            let parsed = ClosedReason::parse(raw).expect("parse");
            assert_eq!(parsed.as_str(), raw);
        }
        assert_eq!(ClosedReason::parse("reopened"), None);
    }

    #[test]
    fn transitions_have_expected_endpoints() {
        assert_eq!(Transition::Start.from_str(), "queued");
        assert_eq!(Transition::Start.to_str(), "in_progress");
        assert_eq!(Transition::Review.to_str(), "review");
        assert_eq!(Transition::Reopen.from_str(), "closed");
        assert_eq!(Transition::Reopen.to_str(), "in_progress");
    }

    #[test]
    fn iso8601_utc_formats_epoch_and_modern() {
        assert_eq!(iso8601_utc(0).unwrap(), "1970-01-01T00:00:00Z");
        assert_eq!(iso8601_utc(1_760_000_000).unwrap(), "2025-10-09T08:53:20Z");
    }
}
