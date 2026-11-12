//! Provider-agnostic chat interface. The only implementation is a raw OpenAI-compatible
//! HTTP client; `ScriptedLlm` is for deterministic tests and evals.

pub mod openai;
pub mod scripted;

use std::future::Future;
use std::pin::Pin;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// Raw JSON string from the provider. Parsed and strictly validated by the application.
    pub arguments_raw: String,
}

impl ToolCall {
    pub fn arguments(&self) -> Result<Value, String> {
        let value: Value = serde_json::from_str(&self.arguments_raw)
            .map_err(|e| format!("arguments for {} are not valid JSON: {e}", self.name))?;
        if !value.is_object() {
            return Err(format!("arguments for {} must be a JSON object", self.name));
        }
        Ok(value)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: Option<String>,
    pub tool_calls: Vec<ToolCall>,
    pub tool_call_id: Option<String>,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".into(),
            content: Some(content.into()),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".into(),
            content: Some(content.into()),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    pub fn assistant(content: Option<String>, tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: "assistant".into(),
            content,
            tool_calls,
            tool_call_id: None,
        }
    }

    pub fn tool(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: "tool".into(),
            content: Some(content.into()),
            tool_calls: Vec::new(),
            tool_call_id: Some(tool_call_id.into()),
        }
    }

    pub fn approximate_chars(&self) -> usize {
        self.content.as_deref().map_or(0, str::len)
            + self
                .tool_calls
                .iter()
                .map(|call| call.name.len() + call.arguments_raw.len())
                .sum::<usize>()
    }
}

#[derive(Debug, Clone, Default)]
pub struct ChatResponse {
    pub content: Option<String>,
    pub tool_calls: Vec<ToolCall>,
}

#[derive(Debug, Error)]
pub enum LlmError {
    #[error("model request failed: {0}")]
    Http(String),
    #[error("model provider error: {0}")]
    Api(String),
    #[error("model response was invalid: {0}")]
    Protocol(String),
}

pub trait LlmProvider: Send + Sync {
    fn chat<'a>(
        &'a self,
        messages: &'a [ChatMessage],
        tools: &'a [Value],
    ) -> Pin<Box<dyn Future<Output = Result<ChatResponse, LlmError>> + Send + 'a>>;
}
