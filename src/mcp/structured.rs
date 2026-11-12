//! Typed contract for the optional `get_planning_snapshot` Canvas MCP tool.
//!
//! This is the preferred data path. It removes dependence on human-readable Canvas text
//! formatting: due dates are RFC 3339 instants, status flags are booleans, and course IDs
//! are explicit. The text parsers remain as a compatibility fallback for existing servers.

use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::Value;

use crate::mcp::parse::ParseError;
use crate::model::{Assignment, Course, WorkState};

pub const CONTRACT_VERSION: &str = "1";
pub const TOOL_NAME: &str = "get_planning_snapshot";

#[derive(Debug, Clone, Deserialize)]
pub struct StructuredSnapshot {
    pub schema_version: String,
    #[serde(default)]
    pub generated_at: Option<String>,
    #[serde(default)]
    pub courses: Vec<StructuredCourse>,
    #[serde(default)]
    pub assignments: Vec<StructuredAssignment>,
    #[serde(default)]
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StructuredCourse {
    pub id: u64,
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub term: Option<String>,
    #[serde(default)]
    pub role: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StructuredAssignment {
    pub course_id: u64,
    pub assignment_id: u64,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub due_at: Option<String>,
    #[serde(default)]
    pub points_possible: Option<f64>,
    #[serde(default)]
    pub submission_types: Vec<String>,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub submission: StructuredSubmission,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct StructuredSubmission {
    #[serde(default)]
    pub workflow_state: Option<String>,
    #[serde(default)]
    pub submitted: bool,
    #[serde(default)]
    pub missing: bool,
    #[serde(default)]
    pub late: bool,
    #[serde(default)]
    pub excused: bool,
    #[serde(default)]
    pub graded: bool,
}

pub fn parse_snapshot(value: &Value) -> Result<StructuredSnapshot, ParseError> {
    let snapshot: StructuredSnapshot = serde_json::from_value(value.clone()).map_err(|error| {
        ParseError::new(format!(
            "structured planning snapshot does not match contract v{CONTRACT_VERSION}: {error}"
        ))
    })?;
    if snapshot.schema_version != CONTRACT_VERSION {
        return Err(ParseError::new(format!(
            "unsupported structured snapshot schema version {:?} (expected {CONTRACT_VERSION})",
            snapshot.schema_version
        )));
    }
    Ok(snapshot)
}

#[derive(Debug, Clone)]
pub struct DomainSnapshot {
    pub courses: Vec<Course>,
    pub assignments: Vec<Assignment>,
    pub warnings: Vec<String>,
}

impl StructuredSnapshot {
    pub fn into_domain(self, now: DateTime<Utc>) -> Result<DomainSnapshot, ParseError> {
        let mut warnings = self.warnings;
        let mut courses = Vec::with_capacity(self.courses.len());
        for course in self.courses {
            let label = course
                .code
                .clone()
                .or_else(|| course.name.clone())
                .unwrap_or_else(|| format!("course {}", course.id));
            courses.push(Course {
                id: course.id,
                code: course.code,
                name: course.name,
                term: course.term,
                role: course.role,
                label,
            });
        }
        let mut by_id = std::collections::HashMap::new();
        for course in &courses {
            by_id.insert(course.id, course.label.clone());
        }

        let mut assignments = Vec::with_capacity(self.assignments.len());
        for assignment in self.assignments {
            let course_label = by_id
                .get(&assignment.course_id)
                .cloned()
                .unwrap_or_else(|| {
                    warnings.push(format!(
                        "Canvas returned assignment {} for course {} without a course record.",
                        assignment.assignment_id, assignment.course_id
                    ));
                    format!("course {}", assignment.course_id)
                });
            let due_at = match assignment.due_at.as_deref() {
                None => None,
                Some(raw) => Some(
                    DateTime::parse_from_rfc3339(raw)
                        .map_err(|error| {
                            ParseError::new(format!(
                                "structured due date {raw:?} for assignment {} is not RFC 3339: {error}",
                                assignment.assignment_id
                            ))
                        })?
                        .with_timezone(&Utc),
                ),
            };
            let state = work_state(&assignment.submission, &assignment.submission_types);
            assignments.push(Assignment {
                course_label,
                course_id: Some(assignment.course_id),
                assignment_id: assignment.assignment_id,
                title: assignment
                    .title
                    .filter(|title| !title.trim().is_empty())
                    .unwrap_or_else(|| "Untitled assignment".into()),
                due_at,
                points_possible: assignment.points_possible,
                state,
                kind: assignment.kind.unwrap_or_else(|| "assignment".into()),
                suspicious: false,
            });
        }
        assignments.sort_by_key(|assignment| {
            (
                assignment.due_at,
                assignment.course_label.clone(),
                assignment.assignment_id,
            )
        });
        let _ = now;
        Ok(DomainSnapshot {
            courses,
            assignments,
            warnings,
        })
    }
}

fn work_state(submission: &StructuredSubmission, submission_types: &[String]) -> WorkState {
    let external = submission_types.iter().any(|kind| kind == "external_tool");
    if external {
        return WorkState::ExternalTool;
    }
    if submission.missing {
        return WorkState::Missing;
    }
    if submission.submitted {
        return if submission.late {
            WorkState::SubmittedLate
        } else {
            WorkState::Submitted
        };
    }
    if submission.excused {
        return WorkState::Excused;
    }
    if submission.graded
        || submission
            .workflow_state
            .as_deref()
            .is_some_and(|state| state == "graded")
    {
        return WorkState::Graded;
    }
    if submission_types.is_empty()
        || submission_types
            .iter()
            .all(|kind| matches!(kind.as_str(), "none" | "on_paper" | "not_graded"))
    {
        return WorkState::NoSubmissionNeeded;
    }
    WorkState::NotSubmitted
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use chrono_tz::America::Chicago;
    use serde_json::json;

    fn now() -> DateTime<Utc> {
        Chicago
            .with_ymd_and_hms(2026, 11, 12, 9, 25, 0)
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn maps_flags_dates_points_and_course_ids() {
        let value = json!({
            "schema_version": "1",
            "courses": [{"id": 101, "code": "CS 101", "name": "Intro", "term": "Fall 2026"}],
            "assignments": [
                {
                    "course_id": 101,
                    "assignment_id": 9002,
                    "title": "Lab 5",
                    "due_at": "2026-11-14T05:59:00Z",
                    "points_possible": 100.0,
                    "submission_types": ["online_upload"],
                    "submission": {"workflow_state": "unsubmitted", "submitted": false, "missing": false}
                },
                {
                    "course_id": 101,
                    "assignment_id": 9001,
                    "title": "Problem set",
                    "due_at": "2026-11-13T05:59:00Z",
                    "points_possible": 50.0,
                    "submission_types": ["online_upload"],
                    "submission": {"missing": true}
                }
            ]
        });
        let parsed = parse_snapshot(&value).unwrap();
        let domain = parsed.into_domain(now()).unwrap();
        assert_eq!(domain.courses[0].label, "CS 101");
        assert!(domain.warnings.is_empty());
        assert_eq!(domain.assignments[0].assignment_id, 9001);
        assert_eq!(domain.assignments[0].state, WorkState::Missing);
        assert_eq!(domain.assignments[1].points_possible, Some(100.0));
        assert_eq!(domain.assignments[1].course_id, Some(101));
        assert_eq!(
            domain.assignments[1].due_at.unwrap().to_rfc3339(),
            "2026-11-14T05:59:00+00:00"
        );
    }

    #[test]
    fn rejects_wrong_schema_and_bad_dates() {
        let wrong = json!({"schema_version": "2", "courses": [], "assignments": []});
        assert!(parse_snapshot(&wrong).is_err());
        let bad = json!({
            "schema_version": "1",
            "courses": [{"id": 1, "code": "X"}],
            "assignments": [{
                "course_id": 1,
                "assignment_id": 2,
                "title": "T",
                "due_at": "Friday",
                "submission": {}
            }]
        });
        let parsed = parse_snapshot(&bad).unwrap();
        assert!(parsed.into_domain(now()).is_err());
    }

    #[test]
    fn external_tool_completion_is_not_guessed() {
        let submission = StructuredSubmission::default();
        assert_eq!(
            work_state(&submission, &["external_tool".into()]),
            WorkState::ExternalTool
        );
        assert_eq!(
            work_state(&submission, &["none".into()]),
            WorkState::NoSubmissionNeeded
        );
        assert_eq!(
            work_state(&submission, &["online_upload".into()]),
            WorkState::NotSubmitted
        );
    }
}
