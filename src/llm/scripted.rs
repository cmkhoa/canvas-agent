//! Deterministic model substitute for integration tests and offline evals.

use std::collections::VecDeque;
use std::sync::Mutex;

use serde::Deserialize;
use serde_json::Value;

use crate::llm::{ChatMessage, ChatResponse, LlmError, LlmProvider, ToolCall};

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ScriptStep {
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub tool_calls: Vec<ScriptToolCall>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ScriptToolCall {
    pub name: String,
    #[serde(default)]
    pub arguments: Value,
}

pub struct ScriptedLlm {
    responses: Mutex<VecDeque<ChatResponse>>,
    remaining: Mutex<usize>,
}

impl ScriptedLlm {
    pub fn new(responses: Vec<ChatResponse>) -> Self {
        let remaining = responses.len();
        Self {
            responses: Mutex::new(responses.into()),
            remaining: Mutex::new(remaining),
        }
    }

    pub fn from_steps(steps: &[ScriptStep]) -> Result<Self, LlmError> {
        let mut responses = Vec::with_capacity(steps.len());
        for (step_index, step) in steps.iter().enumerate() {
            let tool_calls = step
                .tool_calls
                .iter()
                .enumerate()
                .map(|(call_index, call)| {
                    if !call.arguments.is_object() {
                        return Err(LlmError::Protocol(format!(
                            "scripted tool call {} arguments must be an object",
                            call.name
                        )));
                    }
                    Ok(ToolCall {
                        id: format!("script-{step_index}-{call_index}"),
                        name: call.name.clone(),
                        arguments_raw: call.arguments.to_string(),
                    })
                })
                .collect::<Result<Vec<_>, LlmError>>()?;
            responses.push(ChatResponse {
                content: step.text.clone(),
                tool_calls,
            });
        }
        Ok(Self::new(responses))
    }

    pub fn remaining(&self) -> usize {
        *self.remaining.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl LlmProvider for ScriptedLlm {
    fn chat<'a>(
        &'a self,
        _messages: &'a [ChatMessage],
        _tools: &'a [Value],
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<ChatResponse, LlmError>> + Send + 'a>,
    > {
        Box::pin(async move {
            let mut queue = self.responses.lock().unwrap_or_else(|e| e.into_inner());
            let response = queue.pop_front().ok_or_else(|| {
                LlmError::Protocol("scripted model ran out of scripted responses".into())
            })?;
            *self.remaining.lock().unwrap_or_else(|e| e.into_inner()) = queue.len();
            Ok(response)
        })
    }
}
