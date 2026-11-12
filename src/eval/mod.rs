//! Offline evaluation runner: scripted model + mocked Canvas MCP.
//!
//! Evals assert structured outcomes (priority order, citations, refusal reason, audit events,
//! injection flag). They do not judge prose quality and need no Canvas account or model key.

use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use serde::Deserialize;
use serde_json::Value;

use crate::agent::{Agent, TurnResult};
use crate::audit::AuditLog;
use crate::clock::{Clock, FixedClock, parse_clock_override};
use crate::llm::LlmProvider;
use crate::llm::scripted::{ScriptStep, ScriptedLlm};
use crate::mcp::{CanvasMcp, McpError, McpResponse};
use crate::planner::Planner;
use crate::policy::Policy;
use crate::session::Session;

#[derive(Debug, Deserialize)]
pub struct ScenarioFile {
    pub scenarios: Vec<Scenario>,
}

#[derive(Debug, Deserialize)]
pub struct Scenario {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub fixture: String,
    pub now: String,
    pub timezone: String,
    pub input: String,
    #[serde(default = "default_horizon_days")]
    pub horizon_days: u32,
    pub llm: Vec<ScriptStep>,
    #[serde(default)]
    pub expect: Expect,
}

fn default_horizon_days() -> u32 {
    7
}

#[derive(Debug, Default, Deserialize)]
pub struct Expect {
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub must_contain: Vec<String>,
    #[serde(default)]
    pub must_not_contain: Vec<String>,
    #[serde(default)]
    pub audit_events: Vec<String>,
    #[serde(default)]
    pub citation_ids: Vec<String>,
    #[serde(default)]
    pub min_sources: Option<usize>,
    #[serde(default)]
    pub suspicious: Option<bool>,
    #[serde(default)]
    pub refusal_reason: Option<String>,
    #[serde(default)]
    pub priority_order: Vec<u64>,
}

#[derive(Debug, Deserialize)]
pub struct Fixture {
    pub tools: HashMap<String, FixtureResponse>,
}

#[derive(Debug, Deserialize)]
pub struct FixtureResponse {
    #[serde(default)]
    pub ok: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub structured: Option<Value>,
}

#[derive(Debug, Clone)]
pub struct ScenarioResult {
    pub name: String,
    pub passed: bool,
    pub failures: Vec<String>,
    pub output: String,
    pub output_preview: String,
}

#[derive(Debug, Clone)]
pub struct SuiteReport {
    pub results: Vec<ScenarioResult>,
}

impl SuiteReport {
    pub fn passed(&self) -> usize {
        self.results.iter().filter(|result| result.passed).count()
    }

    pub fn failed(&self) -> usize {
        self.results.len() - self.passed()
    }

    pub fn all_passed(&self) -> bool {
        self.failed() == 0
    }

    pub fn print(&self) {
        for result in &self.results {
            if result.passed {
                println!("PASS  {}", result.name);
            } else {
                println!("FAIL  {}", result.name);
                for failure in &result.failures {
                    println!("      - {failure}");
                }
                if !result.output_preview.is_empty() {
                    println!("      output: {}", result.output_preview);
                }
            }
        }
        println!(
            "\n{} passed, {} failed, {} total",
            self.passed(),
            self.failed(),
            self.results.len()
        );
    }

    /// Print each scenario's full answer. Useful for README transcripts and debugging.
    pub fn print_verbose(&self) {
        for result in &self.results {
            println!(
                "===== {} ({}) =====",
                result.name,
                if result.passed { "PASS" } else { "FAIL" }
            );
            for failure in &result.failures {
                println!("FAILURE: {failure}");
            }
            println!("{}", result.output.trim());
            println!();
        }
        println!(
            "{} passed, {} failed, {} total",
            self.passed(),
            self.failed(),
            self.results.len()
        );
    }
}

struct FixtureMcp {
    fixture: Fixture,
}

impl CanvasMcp for FixtureMcp {
    fn call<'a>(
        &'a self,
        tool: &'a str,
        _args: Value,
    ) -> Pin<Box<dyn Future<Output = Result<McpResponse, McpError>> + Send + 'a>> {
        Box::pin(async move {
            let response = self
                .fixture
                .tools
                .get(tool)
                .ok_or_else(|| McpError::NotAllowed(tool.into()))?;
            if let Some(error) = &response.error {
                return Err(McpError::Tool(error.clone()));
            }
            if response.ok.is_none() && response.structured.is_none() {
                return Err(McpError::Protocol(format!(
                    "fixture for {tool} has no ok text or structured content"
                )));
            }
            Ok(McpResponse {
                text: response.ok.clone().unwrap_or_default(),
                structured: response.structured.clone(),
            })
        })
    }

    fn supports_structured_snapshot(&self) -> bool {
        self.fixture.tools.contains_key("get_planning_snapshot")
    }
}

pub async fn run_suite(path: &Path) -> Result<SuiteReport> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("cannot read scenarios {}", path.display()))?;
    let file: ScenarioFile = serde_json::from_str(&raw)
        .with_context(|| format!("invalid scenarios JSON {}", path.display()))?;
    if file.scenarios.is_empty() {
        bail!("scenario file has no scenarios");
    }
    let base = path.parent().unwrap_or_else(|| Path::new("."));
    let policy = Policy::load(&policy_path())?;
    let mut results = Vec::with_capacity(file.scenarios.len());
    for scenario in file.scenarios {
        results.push(run_scenario(&scenario, base, &policy).await?);
    }
    Ok(SuiteReport { results })
}

async fn run_scenario(scenario: &Scenario, base: &Path, policy: &Policy) -> Result<ScenarioResult> {
    let timezone: Tz = scenario
        .timezone
        .parse()
        .map_err(|_| anyhow::anyhow!("scenario {} has an invalid timezone", scenario.name))?;
    let now = parse_scenario_now(&scenario.now, timezone)
        .with_context(|| format!("scenario {}", scenario.name))?;
    let fixture_path = base
        .join("fixtures")
        .join(format!("{}.json", scenario.fixture));
    let fixture_raw = std::fs::read_to_string(&fixture_path)
        .with_context(|| format!("cannot read fixture {}", fixture_path.display()))?;
    let fixture: Fixture = serde_json::from_str(&fixture_raw)
        .with_context(|| format!("invalid fixture {}", fixture_path.display()))?;

    let dir = tempfile::tempdir().context("cannot create eval audit directory")?;
    let audit = AuditLog::open(&dir.path().join("audit.jsonl"))?;
    let planner = Planner::new(policy.clone(), timezone);
    let llm = Arc::new(
        ScriptedLlm::from_steps(&scenario.llm)
            .map_err(|error| anyhow::anyhow!("scenario {} script: {error}", scenario.name))?,
    ) as Arc<dyn LlmProvider>;
    let clock: Arc<dyn Clock> = Arc::new(FixedClock::new(now));
    let mut agent = Agent::new(
        llm,
        Arc::new(FixtureMcp { fixture }),
        clock,
        planner,
        audit,
        Session::new(timezone, scenario.horizon_days),
        6,
        12,
    );
    let result = agent
        .run_turn(&scenario.input)
        .await
        .with_context(|| format!("scenario {} crashed", scenario.name))?;
    let failures = check(&scenario.expect, &result);
    Ok(ScenarioResult {
        name: scenario.name.clone(),
        passed: failures.is_empty(),
        failures,
        output: result.text.clone(),
        output_preview: preview(&result.text),
    })
}

fn check(expect: &Expect, result: &TurnResult) -> Vec<String> {
    let mut failures = Vec::new();
    if let Some(status) = &expect.status {
        match status.as_str() {
            "refusal" if result.refusal.is_none() => {
                failures.push("expected a refusal, got a normal answer".into());
            }
            "ok" if result.refusal.is_some() => {
                failures.push("expected a normal answer, got a refusal".into());
            }
            "refusal" | "ok" => {}
            other => failures.push(format!("unknown expected status {other:?}")),
        }
    }
    if let Some(reason) = &expect.refusal_reason {
        let actual = result.refusal.as_ref().map(|r| r.reason.as_str());
        if actual != Some(reason.as_str()) {
            failures.push(format!(
                "expected refusal reason {reason:?}, got {actual:?}"
            ));
        }
    }
    for text in &expect.must_contain {
        if !result.text.contains(text) {
            failures.push(format!("missing expected text {text:?}"));
        }
    }
    for text in &expect.must_not_contain {
        if result.text.contains(text) {
            failures.push(format!("found forbidden text {text:?}"));
        }
    }
    for event in &expect.audit_events {
        if !result.audit_events.contains(event) {
            failures.push(format!("missing audit event {event:?}"));
        }
    }
    for id in &expect.citation_ids {
        if !result.sources.iter().any(|source| &source.id == id) {
            failures.push(format!("missing citation {id:?}"));
        }
    }
    if let Some(min) = expect.min_sources
        && result.sources.len() < min
    {
        failures.push(format!(
            "expected at least {min} sources, got {}",
            result.sources.len()
        ));
    }
    if let Some(suspicious) = expect.suspicious
        && result.suspicious.is_empty() == suspicious
    {
        failures.push(format!(
            "expected suspicious={suspicious}, got {} labels",
            result.suspicious.len()
        ));
    }
    if !expect.priority_order.is_empty() && result.priority_order != expect.priority_order {
        failures.push(format!(
            "expected priority order {:?}, got {:?}",
            expect.priority_order, result.priority_order
        ));
    }
    failures
}

fn parse_scenario_now(raw: &str, tz: Tz) -> Result<DateTime<Utc>> {
    if let Ok(at) = DateTime::parse_from_rfc3339(raw) {
        return Ok(at.with_timezone(&Utc));
    }
    parse_clock_override(raw, tz).map_err(|error| anyhow::anyhow!(error.to_string()))
}

fn policy_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("config/policy.toml")
}

fn preview(text: &str) -> String {
    let one_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() <= 300 {
        one_line
    } else {
        format!("{}…", one_line.chars().take(300).collect::<String>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scenario_files_are_valid_json() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("evals/scenarios.json");
        let raw = std::fs::read_to_string(&path).unwrap();
        let file: ScenarioFile = serde_json::from_str(&raw).unwrap();
        assert!(file.scenarios.len() >= 8);
        for scenario in &file.scenarios {
            assert!(!scenario.name.is_empty());
            assert!(!scenario.input.is_empty());
            assert!(!scenario.llm.is_empty());
        }
    }
}
