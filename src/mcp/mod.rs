//! A minimal MCP client over stdio, plus the hard allowlist of Canvas tools.
//!
//! Only these four existing canvas-mcp read-only tools are reachable. Write tools and
//! every other Canvas tool are rejected before any JSON-RPC call is made.

pub mod parse;

use std::future::Future;
use std::pin::Pin;

use rmcp::ServiceExt;
use rmcp::model::CallToolRequestParams;
use rmcp::service::{RoleClient, RunningService};
use rmcp::transport::TokioChildProcess;
use serde_json::{Map, Value};
use thiserror::Error;

use crate::config::McpConfig;
use crate::error::ToolError;

pub const ALLOWED_MCP_TOOLS: &[&str] = &[
    "list_courses",
    "get_my_submission_status",
    "get_my_upcoming_assignments",
    "get_assignment_details",
];

#[derive(Debug, Error)]
pub enum McpError {
    #[error("tool {0:?} is not on the Canvas MCP allowlist")]
    NotAllowed(String),
    #[error("could not start canvas-mcp: {0}")]
    Unavailable(String),
    #[error("canvas-mcp returned an error: {0}")]
    Tool(String),
    #[error("canvas-mcp protocol error: {0}")]
    Protocol(String),
}

pub trait CanvasMcp: Send + Sync {
    fn call<'a>(
        &'a self,
        tool: &'a str,
        args: Value,
    ) -> Pin<Box<dyn Future<Output = Result<String, McpError>> + Send + 'a>>;
}

pub fn ensure_allowed(tool: &str) -> Result<(), McpError> {
    if ALLOWED_MCP_TOOLS.contains(&tool) {
        Ok(())
    } else {
        Err(McpError::NotAllowed(tool.to_owned()))
    }
}

pub struct LiveCanvasMcp {
    client: RunningService<RoleClient, ()>,
    command: String,
}

impl LiveCanvasMcp {
    pub async fn connect(config: &McpConfig) -> Result<Self, McpError> {
        let mut command = tokio::process::Command::new(&config.command);
        command.args(&config.args);
        let transport = TokioChildProcess::new(command)
            .map_err(|e| McpError::Unavailable(format!("{}: {e}", config.command)))?;
        let client = ().serve(transport).await.map_err(|e| {
            McpError::Unavailable(format!("MCP handshake with {} failed: {e}", config.command))
        })?;
        Ok(Self {
            client,
            command: config.command.clone(),
        })
    }
}

impl CanvasMcp for LiveCanvasMcp {
    fn call<'a>(
        &'a self,
        tool: &'a str,
        args: Value,
    ) -> Pin<Box<dyn Future<Output = Result<String, McpError>> + Send + 'a>> {
        Box::pin(async move {
            ensure_allowed(tool)?;
            let arguments: Option<Map<String, Value>> = match args {
                Value::Object(map) => Some(map),
                Value::Null => None,
                _ => {
                    return Err(McpError::Protocol(
                        "tool arguments must be a JSON object".into(),
                    ));
                }
            };
            let mut params = CallToolRequestParams::new(tool.to_owned());
            params.arguments = arguments;
            let result = self.client.call_tool(params).await.map_err(|e| {
                McpError::Protocol(format!("{tool} call to {} failed: {e}", self.command))
            })?;
            let text = result
                .content
                .iter()
                .filter_map(|c| c.as_text().map(|t| t.text.clone()))
                .collect::<Vec<_>>()
                .join("\n");
            if result.is_error.unwrap_or(false) {
                Err(McpError::Tool(if text.is_empty() {
                    "canvas-mcp reported an error with no message".into()
                } else {
                    text
                }))
            } else if text.is_empty() {
                Err(McpError::Protocol(format!(
                    "{tool} returned no text content"
                )))
            } else {
                Ok(text)
            }
        })
    }
}

impl From<McpError> for ToolError {
    fn from(error: McpError) -> Self {
        let message = error.to_string();
        match error {
            McpError::NotAllowed(_) => ToolError::new("mcp_tool_not_allowed", message),
            McpError::Unavailable(_) | McpError::Protocol(_) => {
                ToolError::retryable("mcp_unavailable", message)
            }
            McpError::Tool(_) => ToolError::retryable("mcp_tool_error", message),
        }
    }
}

/// Kept private so tests exercise the public guard rather than this helper.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_read_only_canvas_tools_are_allowed() {
        for tool in ALLOWED_MCP_TOOLS {
            assert!(ensure_allowed(tool).is_ok());
        }
        for tool in [
            "submit_assignment",
            "delete_page",
            "update_grade",
            "get_my_profile",
            "",
        ] {
            assert!(matches!(ensure_allowed(tool), Err(McpError::NotAllowed(_))));
        }
    }
}
