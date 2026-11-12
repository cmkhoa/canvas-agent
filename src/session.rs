//! Minimal explicit session state. Timezone and horizon are set by the user or CLI,
//! never inferred from chat text. Identity and permissions are always Canvas's.

use chrono_tz::Tz;

use crate::error::{AgentError, Result};
use crate::llm::ChatMessage;
use crate::model::{Citation, PlanMemory};

const MAX_HISTORY_MESSAGES: usize = 10;
const MAX_HISTORY_CHARS: usize = 16_000;

#[derive(Debug)]
pub struct Session {
    pub timezone: Tz,
    pub horizon_days: u32,
    pub turn: u32,
    pub history: Vec<ChatMessage>,
    pub last_plan: Option<PlanMemory>,
    pub last_sources: Vec<Citation>,
}

impl Session {
    pub fn new(timezone: Tz, horizon_days: u32) -> Self {
        Self {
            timezone,
            horizon_days,
            turn: 0,
            history: Vec::new(),
            last_plan: None,
            last_sources: Vec::new(),
        }
    }

    pub fn next_turn(&mut self) -> u32 {
        self.turn += 1;
        self.turn
    }

    pub fn set_horizon(&mut self, days: u32, max: u32) -> Result<()> {
        if !(1..=max).contains(&days) {
            return Err(AgentError::Config(format!(
                "horizon must be between 1 and {max} days"
            )));
        }
        self.horizon_days = days;
        Ok(())
    }

    pub fn record_exchange(&mut self, user: &str, assistant: &str) {
        self.history.push(ChatMessage::user(user.to_string()));
        self.history
            .push(ChatMessage::assistant(Some(compact(assistant)), Vec::new()));
        self.trim_history();
    }

    pub fn record_assistant_only(&mut self, assistant: &str) {
        self.history
            .push(ChatMessage::assistant(Some(compact(assistant)), Vec::new()));
        self.trim_history();
    }

    pub fn clear_history(&mut self) {
        self.history.clear();
        self.last_plan = None;
        self.last_sources.clear();
    }

    fn trim_history(&mut self) {
        while self.history.len() > MAX_HISTORY_MESSAGES {
            self.history.remove(0);
        }
        while self.total_chars() > MAX_HISTORY_CHARS && self.history.len() > 2 {
            self.history.remove(0);
        }
        // Keep the history starting with a user turn where possible.
        while self
            .history
            .first()
            .is_some_and(|message| message.role != "user")
            && self.history.len() > 1
        {
            self.history.remove(0);
        }
    }

    fn total_chars(&self) -> usize {
        self.history
            .iter()
            .map(ChatMessage::approximate_chars)
            .sum()
    }

    /// Compact memory included in the next system prompt, not the transcript.
    pub fn plan_memory_prompt(&self) -> Option<String> {
        let plan = self.last_plan.as_ref()?;
        if plan.items.is_empty() {
            return None;
        }
        let items = plan
            .items
            .iter()
            .take(8)
            .map(|(id, title)| format!("assignment {id} \"{title}\""))
            .collect::<Vec<_>>()
            .join("; ");
        Some(format!(
            "Previous plan ({}) included: {items}. Use this only to resolve follow-ups such as \
             \"the second one\"; call a tool again for current facts.",
            plan.horizon_label
        ))
    }
}

fn compact(text: &str) -> String {
    let max = 2000;
    if text.chars().count() <= max {
        text.to_string()
    } else {
        let mut out: String = text.chars().take(max).collect();
        out.push_str("\n[truncated in session history]");
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono_tz::America::Chicago;

    #[test]
    fn trims_history_to_a_bounded_window() {
        let mut session = Session::new(Chicago, 7);
        for index in 0..20 {
            session.record_exchange(
                &format!("question {index}"),
                &format!("answer {index} {}", "x".repeat(3000)),
            );
        }
        assert!(session.history.len() <= MAX_HISTORY_MESSAGES);
        assert!(session.total_chars() <= MAX_HISTORY_CHARS);
        assert_eq!(session.history[0].role, "user");
    }

    #[test]
    fn horizon_is_validated_against_policy_max() {
        let mut session = Session::new(Chicago, 7);
        assert!(session.set_horizon(14, 30).is_ok());
        assert_eq!(session.horizon_days, 14);
        assert!(session.set_horizon(0, 30).is_err());
        assert!(session.set_horizon(31, 30).is_err());
    }

    #[test]
    fn plan_memory_is_compact_and_does_not_include_dates() {
        let mut session = Session::new(Chicago, 7);
        session.last_plan = Some(PlanMemory {
            horizon_label: "next 7 days".into(),
            items: vec![(1, "Lab".into()), (2, "Essay".into())],
        });
        let prompt = session.plan_memory_prompt().unwrap();
        assert!(prompt.contains("assignment 1"));
        assert!(!prompt.contains("2026-"));
    }
}
