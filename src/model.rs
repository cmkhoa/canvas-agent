//! Domain types shared by the planner, tools, and renderer.
//!
//! Canvas free text is stored as data only. Nothing in this module interprets
//! an assignment title or description as an instruction.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use serde::Serialize;

use crate::clock::format_short;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkState {
    Missing,
    NotSubmitted,
    SubmittedLate,
    Submitted,
    Graded,
    Excused,
    ExternalTool,
    NoSubmissionNeeded,
    Unknown,
}

impl WorkState {
    pub fn is_done(self) -> bool {
        matches!(
            self,
            Self::Submitted
                | Self::SubmittedLate
                | Self::Graded
                | Self::Excused
                | Self::NoSubmissionNeeded
        )
    }

    pub fn is_open(self) -> bool {
        matches!(self, Self::Missing | Self::NotSubmitted | Self::Unknown)
    }

    pub fn needs_verification(self) -> bool {
        matches!(self, Self::ExternalTool)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Missing => "Canvas flags missing",
            Self::NotSubmitted => "not submitted",
            Self::SubmittedLate => "submitted late",
            Self::Submitted => "submitted",
            Self::Graded => "graded",
            Self::Excused => "excused",
            Self::ExternalTool => "external tool — completion unknown",
            Self::NoSubmissionNeeded => "no submission needed",
            Self::Unknown => "status unknown",
        }
    }

    /// Higher is more urgent for the status component of the priority score.
    pub fn status_weight_key(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::NotSubmitted => "not_submitted",
            Self::Unknown => "unknown",
            _ => "none",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Course {
    pub id: u64,
    pub code: Option<String>,
    pub name: Option<String>,
    pub term: Option<String>,
    pub role: Option<String>,
    pub label: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Assignment {
    pub course_label: String,
    pub course_id: Option<u64>,
    pub assignment_id: u64,
    pub title: String,
    pub due_at: Option<DateTime<Utc>>,
    pub points_possible: Option<f64>,
    pub state: WorkState,
    pub kind: String,
    /// True when the title or source text contained prompt-injection-like content.
    pub suspicious: bool,
}

impl Assignment {
    pub fn is_open(&self) -> bool {
        self.state.is_open()
    }

    /// Number of days overdue, floored at 0. Uses exact instants, not calendar days.
    pub fn overdue_days(&self, now: DateTime<Utc>) -> i64 {
        match self.due_at {
            Some(due) if due < now => (now - due).num_days().max(0),
            _ => 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct SourceKey {
    course_id: Option<u64>,
    course_label: String,
    assignment_id: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Citation {
    pub id: String,
    pub course: String,
    pub course_id: Option<u64>,
    pub assignment_id: Option<u64>,
    pub title: Option<String>,
    pub due_at: Option<DateTime<Utc>>,
}

impl Citation {
    pub fn render(&self, tz: Tz) -> String {
        let mut out = format!("[{}] {}", self.id, self.course);
        match self.course_id {
            Some(id) => out.push_str(&format!(" (course {id})")),
            None => out.push_str(" (course ID not returned by Canvas)"),
        }
        if let Some(id) = self.assignment_id {
            let title = self.title.as_deref().unwrap_or("Untitled assignment");
            out.push_str(&format!(" — \"{title}\" (assignment {id})"));
            match self.due_at {
                Some(due) => out.push_str(&format!(", due {}", format_short(due, tz))),
                None => out.push_str(", due date not provided by Canvas"),
            }
        }
        out
    }
}

/// Per-turn citation registry. IDs are assigned deterministically in first-use order.
#[derive(Debug, Clone)]
pub struct CitationRegistry {
    tz: Tz,
    sources: Vec<Citation>,
    index: HashMap<SourceKey, usize>,
}

impl CitationRegistry {
    pub fn new(tz: Tz) -> Self {
        Self {
            tz,
            sources: Vec::new(),
            index: HashMap::new(),
        }
    }

    pub fn tz(&self) -> Tz {
        self.tz
    }

    pub fn cite_assignment(&mut self, assignment: &Assignment) -> String {
        let key = SourceKey {
            course_id: assignment.course_id,
            course_label: assignment.course_label.clone(),
            assignment_id: Some(assignment.assignment_id),
        };
        if let Some(existing) = self.index.get(&key) {
            return self.sources[*existing].id.clone();
        }
        let id = format!("S{}", self.sources.len() + 1);
        self.sources.push(Citation {
            id: id.clone(),
            course: assignment.course_label.clone(),
            course_id: assignment.course_id,
            assignment_id: Some(assignment.assignment_id),
            title: Some(assignment.title.clone()),
            due_at: assignment.due_at,
        });
        self.index.insert(key, self.sources.len() - 1);
        id
    }

    pub fn cite_course(&mut self, course: &Course) -> String {
        let key = SourceKey {
            course_id: Some(course.id),
            course_label: course.label.clone(),
            assignment_id: None,
        };
        if let Some(existing) = self.index.get(&key) {
            return self.sources[*existing].id.clone();
        }
        let id = format!("S{}", self.sources.len() + 1);
        self.sources.push(Citation {
            id: id.clone(),
            course: course.label.clone(),
            course_id: Some(course.id),
            assignment_id: None,
            title: None,
            due_at: None,
        });
        self.index.insert(key, self.sources.len() - 1);
        id
    }

    pub fn all(&self) -> &[Citation] {
        &self.sources
    }

    pub fn render_sources(&self) -> String {
        if self.sources.is_empty() {
            return String::new();
        }
        let mut out = String::from("Sources (Canvas via canvas-mcp):\n");
        for source in &self.sources {
            out.push_str(&source.render(self.tz));
            out.push('\n');
        }
        out
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RefusalReason {
    WriteActionsNotSupported,
    OutOfScope,
    CanvasUnavailable,
    AmbiguousRequest,
    InvalidRequest,
}

impl RefusalReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::WriteActionsNotSupported => "write_actions_not_supported",
            Self::OutOfScope => "out_of_scope",
            Self::CanvasUnavailable => "canvas_unavailable",
            Self::AmbiguousRequest => "ambiguous_request",
            Self::InvalidRequest => "invalid_request",
        }
    }

    pub fn message(self) -> &'static str {
        match self {
            Self::WriteActionsNotSupported => {
                "I can't make changes in Canvas: this planner is read-only. It cannot submit, \
                 edit, delete, message, enroll, or change a grade. I can still help you plan the \
                 work and show what Canvas says is due."
            }
            Self::OutOfScope => {
                "I can only help with planning your Canvas coursework from the data canvas-mcp \
                 can read. I can't answer that from Canvas. Ask me what's due, what to prioritize, \
                 or how your workload breaks down by course."
            }
            Self::CanvasUnavailable => {
                "I couldn't read your Canvas data, so I won't guess. Check that canvas-mcp is \
                 configured and signed in, then ask again."
            }
            Self::AmbiguousRequest => {
                "I couldn't resolve the planning window unambiguously. Give me a number of days, \
                 a weekday such as Friday, or a date in YYYY-MM-DD form."
            }
            Self::InvalidRequest => {
                "I can't act on that request. Try asking about your deadlines, priorities, or \
                 workload by course."
            }
        }
    }

    pub fn follow_up(self) -> &'static str {
        match self {
            Self::WriteActionsNotSupported => {
                "Would you like me to show what's due next so you can do it in Canvas yourself?"
            }
            Self::OutOfScope => "Would you like a plan for the work Canvas shows as upcoming?",
            Self::CanvasUnavailable => "Would you like me to retry once canvas-mcp is available?",
            Self::AmbiguousRequest => "Should I use the next 7 days?",
            Self::InvalidRequest => "Should I show what's due in the next 7 days?",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Refusal {
    pub reason: RefusalReason,
    pub message: String,
}

impl Refusal {
    pub fn new(reason: RefusalReason) -> Self {
        Self {
            reason,
            message: format!("{}\n\n{}", reason.message(), reason.follow_up()),
        }
    }
}

/// A compact, code-generated memory of the last plan so follow-ups can refer to it.
#[derive(Debug, Clone, Default)]
pub struct PlanMemory {
    pub horizon_label: String,
    pub items: Vec<(u64, String)>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use chrono_tz::America::Chicago;

    fn assignment(id: u64, course: &str, due: Option<DateTime<Utc>>) -> Assignment {
        Assignment {
            course_label: course.into(),
            course_id: Some(101),
            assignment_id: id,
            title: format!("Work {id}"),
            due_at: due,
            points_possible: Some(10.0),
            state: WorkState::NotSubmitted,
            kind: "assignment".into(),
            suspicious: false,
        }
    }

    #[test]
    fn citations_are_deduplicated_in_first_use_order() {
        let mut registry = CitationRegistry::new(Chicago);
        let a = assignment(1, "CS 101", None);
        let b = assignment(2, "CS 101", None);
        assert_eq!(registry.cite_assignment(&a), "S1");
        assert_eq!(registry.cite_assignment(&a), "S1");
        assert_eq!(registry.cite_assignment(&b), "S2");
        assert_eq!(registry.all().len(), 2);
    }

    #[test]
    fn citations_name_course_id_assignment_and_missing_due_date() {
        let mut registry = CitationRegistry::new(Chicago);
        let mut a = assignment(7, "CS 101", None);
        a.course_id = None;
        let id = registry.cite_assignment(&a);
        let text = registry.render_sources();
        assert!(text.contains(&format!("[{id}] CS 101")));
        assert!(text.contains("course ID not returned"));
        assert!(text.contains("assignment 7"));
        assert!(text.contains("due date not provided by Canvas"));
    }

    #[test]
    fn source_rendering_uses_the_student_zone() {
        let due = Chicago
            .with_ymd_and_hms(2026, 11, 14, 23, 59, 0)
            .unwrap()
            .with_timezone(&Utc);
        let mut registry = CitationRegistry::new(Chicago);
        registry.cite_assignment(&assignment(3, "CS 101", Some(due)));
        let text = registry.render_sources();
        assert!(text.contains("Sat Nov 14, 2026, 11:59 PM CST"), "{text}");
    }

    #[test]
    fn work_states_split_open_done_and_verification() {
        assert!(WorkState::Missing.is_open());
        assert!(WorkState::Submitted.is_done());
        assert!(WorkState::ExternalTool.needs_verification());
        assert!(!WorkState::ExternalTool.is_open());
    }

    #[test]
    fn overdue_days_are_floor_days() {
        let now = Chicago
            .with_ymd_and_hms(2026, 11, 12, 9, 0, 0)
            .unwrap()
            .with_timezone(&Utc);
        let due = now - chrono::Duration::hours(30);
        let a = assignment(1, "CS 101", Some(due));
        assert_eq!(a.overdue_days(now), 1);
    }
}
