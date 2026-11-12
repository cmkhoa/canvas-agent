//! Code-owned checks on the model's narrative. The deterministic appendix is not validated
//! here because it is rendered by code after these checks.

use crate::model::Citation;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationReport {
    pub cited: Vec<String>,
    pub issues: Vec<String>,
}

impl ValidationReport {
    pub fn ok(&self) -> bool {
        self.issues.is_empty()
    }
}

/// Extracts `S1`, `S2` from forms such as `[S1]`, `[S1, S2]`, and `[S1][S2]`.
pub fn extract_citations(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'[' {
            index += 1;
            continue;
        }
        let Some(close_rel) = text[index + 1..].find(']') else {
            break;
        };
        let close = index + 1 + close_rel;
        let inner = &text[index + 1..close];
        for token in inner.split([',', ';', ' ']) {
            let token = token.trim();
            if let Some(rest) = token.strip_prefix('S')
                && !rest.is_empty()
                && rest.chars().all(|c| c.is_ascii_digit())
                && !out.iter().any(|id| id == token)
            {
                out.push(token.to_string());
            }
        }
        index = close + 1;
    }
    out
}

/// A deliberately broad, code-owned guard: a planning question must either call a Canvas
/// tool or be answered as unavailable. It is not an intent classifier; the model still
/// interprets the request.
pub fn looks_like_canvas_question(input: &str) -> bool {
    let lower = input.to_ascii_lowercase();
    const TERMS: &[&str] = &[
        "assignment",
        "due",
        "deadline",
        "priorit",
        "workload",
        "course",
        "class",
        "submit",
        "finish",
        "risk",
        "exam",
        "quiz",
        "project",
        "grade",
        "canvas",
        "syllabus",
        "module",
        "announcement",
        "inbox",
        "calendar",
        "week",
        "today",
        "tomorrow",
        "monday",
        "tuesday",
        "wednesday",
        "thursday",
        "friday",
        "saturday",
        "sunday",
        "what do i need",
        "what should i",
        "my work",
    ];
    TERMS.iter().any(|term| lower.contains(term))
}

pub fn validate_answer_with_context(
    text: &str,
    sources: &[Citation],
    tools_used: bool,
    canvas_required: bool,
    canvas_attempted: bool,
) -> ValidationReport {
    let mut report = validate_answer(text, sources, tools_used);
    if canvas_required && !tools_used && !canvas_attempted {
        report.issues.push(
            "This is a Canvas planning question, but no Canvas tool was called. Call a tool or              state clearly that Canvas data is unavailable."
                .to_string(),
        );
    }
    report
}

pub fn validate_answer(text: &str, sources: &[Citation], tools_used: bool) -> ValidationReport {
    let mut issues = Vec::new();
    let cited = extract_citations(text);
    let known: Vec<&str> = sources.iter().map(|source| source.id.as_str()).collect();

    if tools_used && !sources.is_empty() && cited.is_empty() {
        issues.push(
            "The answer uses Canvas data but cites no sources. Cite at least one [S#] source."
                .to_string(),
        );
    }
    if !tools_used && !cited.is_empty() {
        issues.push(
            "The answer cites Canvas sources, but no Canvas tool succeeded in this turn."
                .to_string(),
        );
    }
    for id in &cited {
        if !known.contains(&id.as_str()) {
            issues.push(format!(
                "Citation [{id}] does not exist. Valid sources: {}.",
                if known.is_empty() {
                    "none".to_string()
                } else {
                    known.join(", ")
                }
            ));
        }
    }
    if text.chars().count() > 6000 {
        issues.push("The answer is longer than 6000 characters.".to_string());
    }
    for claim in forbidden_claims(text) {
        issues.push(format!(
            "The answer makes an unsupported claim ({claim:?}). Do not claim to change Canvas, \
             predict a grade, or guarantee an outcome."
        ));
    }
    if text.trim().is_empty() {
        issues.push("The answer is empty.".to_string());
    }
    ValidationReport { cited, issues }
}

fn forbidden_claims(text: &str) -> Vec<&'static str> {
    let lower = text.to_ascii_lowercase();
    const CLAIMS: &[&str] = &[
        "your grade will",
        "you will fail",
        "guaranteed",
        "i have submitted",
        "i submitted your",
        "i changed your",
        "i deleted",
        "i emailed",
        "i have enrolled",
    ];
    CLAIMS
        .iter()
        .copied()
        .filter(|claim| lower.contains(claim))
        .collect()
}

pub fn repair_instruction(issues: &[String], sources: &[Citation]) -> String {
    let source_list = if sources.is_empty() {
        "No Canvas sources were retrieved.".to_string()
    } else {
        sources
            .iter()
            .map(|source| {
                format!(
                    "[{}] {} / assignment {:?}",
                    source.id, source.course, source.assignment_id
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    format!(
        "Your previous answer failed validation:\n- {}\n\nRewrite it once. Use only the facts \
         in the tool results. Cite Canvas facts with the exact [S#] IDs below. Do not add a \
         Sources section; the application appends it. Keep it under 4000 characters.\n\n{}",
        issues.join("\n- "),
        source_list
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sources() -> Vec<Citation> {
        vec![
            Citation {
                id: "S1".into(),
                course: "CS 101".into(),
                course_id: Some(101),
                assignment_id: Some(1),
                title: Some("Lab".into()),
                due_at: None,
            },
            Citation {
                id: "S2".into(),
                course: "BIO 200".into(),
                course_id: Some(202),
                assignment_id: Some(2),
                title: Some("Essay".into()),
                due_at: None,
            },
        ]
    }

    #[test]
    fn extracts_single_and_grouped_citations() {
        assert_eq!(
            extract_citations("Do [S1] then [S2], compare [S1, S2] and [S9]."),
            vec!["S1", "S2", "S9"]
        );
    }

    #[test]
    fn requires_a_citation_when_tools_were_used() {
        let report = validate_answer("Here is your plan.", &sources(), true);
        assert!(!report.ok());
        assert!(report.issues[0].contains("cites no sources"));
    }

    #[test]
    fn rejects_unknown_and_phantom_citations() {
        let report = validate_answer("See [S9].", &sources(), true);
        assert!(report.issues.iter().any(|i| i.contains("does not exist")));
        let report = validate_answer("See [S1].", &sources(), false);
        assert!(report.issues.iter().any(|i| i.contains("no Canvas tool")));
    }

    #[test]
    fn empty_canvas_results_do_not_require_a_citation() {
        let report = validate_answer("Nothing is due in this window.", &[], true);
        assert!(report.ok(), "{report:?}");
    }

    #[test]
    fn accepts_grounded_answers() {
        let report = validate_answer("Start with [S1], then [S2].", &sources(), true);
        assert!(report.ok(), "{report:?}");
    }

    #[test]
    fn blocks_write_and_grade_predictions() {
        let report = validate_answer(
            "I submitted your essay. Your grade will improve.",
            &sources(),
            false,
        );
        assert!(!report.ok());
        assert!(
            report
                .issues
                .iter()
                .any(|i| i.contains("unsupported claim"))
        );
    }
}
