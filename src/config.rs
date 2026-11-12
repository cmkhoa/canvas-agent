//! Environment and CLI configuration. Secrets live only in memory and are never debug-printed.

use std::env;
use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

use chrono::{DateTime, Utc};
use chrono_tz::Tz;

use crate::clock::parse_clock_override;
use crate::error::{AgentError, Result};

#[derive(Debug, Default, Clone)]
pub struct Overrides {
    pub timezone: Option<String>,
    pub now: Option<String>,
    pub horizon_days: Option<u32>,
    pub audit_log: Option<PathBuf>,
    pub policy: Option<PathBuf>,
    pub mcp_command: Option<String>,
    pub mcp_args: Option<Vec<String>>,
    pub model: Option<String>,
    pub base_url: Option<String>,
    pub max_steps: Option<usize>,
    pub max_tool_calls: Option<usize>,
}

#[derive(Clone)]
pub struct LlmConfig {
    pub api_key: String,
    pub base_url: String,
    pub model: String,
    pub temperature: f32,
    pub timeout: Duration,
    pub max_retries: u32,
}

#[derive(Debug, Clone)]
pub struct McpConfig {
    pub command: String,
    pub args: Vec<String>,
}

#[derive(Clone)]
pub struct AppConfig {
    pub llm: LlmConfig,
    pub mcp: McpConfig,
    pub timezone: Tz,
    pub now_override: Option<DateTime<Utc>>,
    pub horizon_days: u32,
    pub audit_path: PathBuf,
    pub policy_path: PathBuf,
    pub max_tool_steps: usize,
    pub max_tool_calls: usize,
}

impl fmt::Debug for AppConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AppConfig")
            .field("llm_base_url", &self.llm.base_url)
            .field("llm_model", &self.llm.model)
            .field("llm_api_key", &"<redacted>")
            .field("mcp", &self.mcp)
            .field("timezone", &self.timezone.name())
            .field("now_override", &self.now_override)
            .field("horizon_days", &self.horizon_days)
            .field("audit_path", &self.audit_path)
            .field("policy_path", &self.policy_path)
            .field("max_tool_steps", &self.max_tool_steps)
            .field("max_tool_calls", &self.max_tool_calls)
            .finish()
    }
}

impl AppConfig {
    /// Loads `.env` exactly once at process start (before clap parses env-backed flags).
    pub fn load_dotenv() {
        let _ = dotenvy::dotenv();
    }

    /// `require_llm_key` is false for the offline evaluation runner.
    pub fn load(overrides: Overrides, require_llm_key: bool) -> Result<Self> {
        let timezone_raw = overrides
            .timezone
            .or_else(|| env_opt("STUDENT_TIMEZONE"))
            .ok_or_else(|| {
                AgentError::Config(
                    "STUDENT_TIMEZONE is required (an IANA zone such as America/Chicago). \
                     Set it in .env or pass --timezone."
                        .into(),
                )
            })?;
        let timezone: Tz = timezone_raw.parse().map_err(|_| {
            AgentError::Config(format!(
                "STUDENT_TIMEZONE={timezone_raw:?} is not an IANA time zone \
                 (example: America/Chicago)"
            ))
        })?;

        let now_override = overrides
            .now
            .or_else(|| env_opt("CANVAS_AGENT_NOW"))
            .filter(|s| !s.trim().is_empty())
            .map(|raw| parse_clock_override(&raw, timezone))
            .transpose()?;

        let horizon_days = overrides
            .horizon_days
            .unwrap_or_else(|| env_parse("CANVAS_AGENT_HORIZON_DAYS", 7));
        if !(1..=365).contains(&horizon_days) {
            return Err(AgentError::Config(
                "planning horizon must be between 1 and 365 days".into(),
            ));
        }

        let audit_path = overrides
            .audit_log
            .or_else(|| env_opt("CANVAS_AGENT_AUDIT_LOG").map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from("logs/audit.jsonl"));
        let policy_path = overrides
            .policy
            .or_else(|| env_opt("CANVAS_AGENT_POLICY").map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from("config/policy.toml"));

        let mcp_command = overrides
            .mcp_command
            .or_else(|| env_opt("CANVAS_MCP_COMMAND"))
            .unwrap_or_else(|| "canvas-mcp".into());
        if mcp_command.trim().is_empty() {
            return Err(AgentError::Config("CANVAS_MCP_COMMAND is empty".into()));
        }
        let mcp_args = overrides.mcp_args.unwrap_or_else(|| {
            env_opt("CANVAS_MCP_ARGS")
                .map(|s| s.split_whitespace().map(str::to_owned).collect())
                .unwrap_or_default()
        });

        let api_key = env_opt("LLM_API_KEY").unwrap_or_default();
        if require_llm_key {
            if api_key.trim().is_empty() {
                return Err(AgentError::Config(
                    "LLM_API_KEY is required for chat (not needed for `canvas-eval`)".into(),
                ));
            }
            if api_key.contains("replace-with") {
                return Err(AgentError::Config(
                    "LLM_API_KEY still contains the placeholder; edit .env".into(),
                ));
            }
        }

        let base_url = overrides
            .base_url
            .or_else(|| env_opt("LLM_BASE_URL"))
            .unwrap_or_else(|| "https://api.openai.com/v1".into())
            .trim_end_matches('/')
            .to_owned();
        validate_base_url(&base_url)?;

        let model = overrides
            .model
            .or_else(|| env_opt("LLM_MODEL"))
            .unwrap_or_else(|| "gpt-4.1-mini".into());
        if model.trim().is_empty() {
            return Err(AgentError::Config("LLM_MODEL is empty".into()));
        }

        let temperature = env_opt("LLM_TEMPERATURE")
            .map(|s| {
                s.parse::<f32>().map_err(|_| {
                    AgentError::Config("LLM_TEMPERATURE must be a number from 0 to 2".into())
                })
            })
            .transpose()?
            .unwrap_or(0.2);
        if !(0.0..=2.0).contains(&temperature) {
            return Err(AgentError::Config(
                "LLM_TEMPERATURE must be between 0 and 2".into(),
            ));
        }

        let timeout_secs = env_parse("LLM_TIMEOUT_SECS", 60_u64);
        if !(5..=600).contains(&timeout_secs) {
            return Err(AgentError::Config(
                "LLM_TIMEOUT_SECS must be between 5 and 600".into(),
            ));
        }

        let max_retries = env_parse("LLM_MAX_RETRIES", 2_u32);
        if max_retries > 5 {
            return Err(AgentError::Config("LLM_MAX_RETRIES must be 0..=5".into()));
        }

        let max_tool_steps = overrides
            .max_steps
            .unwrap_or_else(|| env_parse("CANVAS_AGENT_MAX_STEPS", 6_usize));
        if !(1..=12).contains(&max_tool_steps) {
            return Err(AgentError::Config(
                "CANVAS_AGENT_MAX_STEPS must be between 1 and 12".into(),
            ));
        }
        let max_tool_calls = overrides
            .max_tool_calls
            .unwrap_or_else(|| env_parse("CANVAS_AGENT_MAX_TOOL_CALLS", 12_usize));
        if !(1..=30).contains(&max_tool_calls) {
            return Err(AgentError::Config(
                "CANVAS_AGENT_MAX_TOOL_CALLS must be between 1 and 30".into(),
            ));
        }

        Ok(Self {
            llm: LlmConfig {
                api_key,
                base_url,
                model,
                temperature,
                timeout: Duration::from_secs(timeout_secs),
                max_retries,
            },
            mcp: McpConfig {
                command: mcp_command,
                args: mcp_args,
            },
            timezone,
            now_override,
            horizon_days,
            audit_path,
            policy_path,
            max_tool_steps,
            max_tool_calls,
        })
    }
}

fn env_opt(key: &str) -> Option<String> {
    env::var(key).ok().filter(|s| !s.trim().is_empty())
}

fn env_parse<T: std::str::FromStr>(key: &str, default: T) -> T {
    env_opt(key)
        .and_then(|s| s.parse::<T>().ok())
        .unwrap_or(default)
}

fn validate_base_url(base_url: &str) -> Result<()> {
    let https = base_url.starts_with("https://");
    let local_http = base_url.starts_with("http://127.0.0.1")
        || base_url.starts_with("http://localhost")
        || base_url.starts_with("http://[::1]");
    if !https && !local_http {
        return Err(AgentError::Config(format!(
            "LLM_BASE_URL must use https:// (plain http is allowed only for localhost), got {base_url:?}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono_tz::America::Chicago;

    fn base(require_key: bool) -> Result<AppConfig> {
        AppConfig::load(
            Overrides {
                timezone: Some("America/Chicago".into()),
                now: Some("2026-11-12T09:25:00-06:00".into()),
                mcp_command: Some("canvas-mcp".into()),
                ..Default::default()
            },
            require_key,
        )
    }

    #[test]
    fn loads_without_a_key_for_evals_and_parses_the_clock() {
        let config = base(false).unwrap();
        assert_eq!(config.timezone, Chicago);
        assert_eq!(
            config.now_override.unwrap().to_rfc3339(),
            "2026-11-12T15:25:00+00:00"
        );
        assert_eq!(config.llm.api_key, "");
    }

    #[test]
    fn rejects_plain_http_except_locally() {
        assert!(validate_base_url("http://example.com/v1").is_err());
        assert!(validate_base_url("http://127.0.0.1:8080/v1").is_ok());
        assert!(validate_base_url("https://api.openai.com/v1").is_ok());
    }

    #[test]
    fn unknown_timezone_is_a_clear_error() {
        let err = AppConfig::load(
            Overrides {
                timezone: Some("Mars/Olympus".into()),
                ..Default::default()
            },
            false,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("not an IANA time zone"), "{err}");
    }

    #[test]
    fn debug_never_prints_the_api_key() {
        let mut config = base(false).unwrap();
        config.llm.api_key = "super-secret-key".into();
        let debug = format!("{config:?}");
        assert!(!debug.contains("super-secret-key"), "{debug}");
        assert!(debug.contains("<redacted>"));
    }
}
