//! Minimal raw OpenAI-compatible chat-completions client.
//!
//! No SDK and no agent framework. The API key is only used in the Authorization header;
//! it is never logged, echoed in errors, or included in the audit trail.

use std::time::Duration;

use reqwest::Client;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::config::LlmConfig;
use crate::llm::{ChatMessage, ChatResponse, LlmError, LlmProvider, ToolCall};

pub struct OpenAiClient {
    http: Client,
    api_key: String,
    base_url: String,
    model: String,
    temperature: f32,
    max_retries: u32,
}

impl OpenAiClient {
    pub fn new(config: &LlmConfig) -> Result<Self, LlmError> {
        let http = Client::builder()
            .timeout(config.timeout)
            .build()
            .map_err(|e| LlmError::Protocol(format!("cannot build HTTP client: {e}")))?;
        Ok(Self {
            http,
            api_key: config.api_key.clone(),
            base_url: config.base_url.trim_end_matches('/').to_string(),
            model: config.model.clone(),
            temperature: config.temperature,
            max_retries: config.max_retries,
        })
    }

    fn endpoint(&self) -> String {
        format!("{}/chat/completions", self.base_url)
    }

    async fn post_once(&self, body: &Value) -> Result<(reqwest::StatusCode, String), LlmError> {
        let response = self
            .http
            .post(self.endpoint())
            .bearer_auth(&self.api_key)
            .header("User-Agent", "canvas-agent/0.1")
            .json(body)
            .send()
            .await
            .map_err(|e| LlmError::Http(sanitize_error(&e.to_string())))?;
        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|e| LlmError::Http(sanitize_error(&e.to_string())))?;
        Ok((status, text))
    }
}

impl LlmProvider for OpenAiClient {
    fn chat<'a>(
        &'a self,
        messages: &'a [ChatMessage],
        tools: &'a [Value],
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<ChatResponse, LlmError>> + Send + 'a>,
    > {
        Box::pin(async move {
            let mut body = Map::new();
            body.insert("model".into(), json!(self.model));
            body.insert(
                "messages".into(),
                Value::Array(messages.iter().map(message_json).collect()),
            );
            body.insert("temperature".into(), json!(self.temperature));
            body.insert("stream".into(), json!(false));
            if !tools.is_empty() {
                body.insert("tools".into(), Value::Array(tools.to_vec()));
                body.insert("tool_choice".into(), json!("auto"));
            }

            let mut attempt = 0_u32;
            loop {
                let (status, text) = self.post_once(&Value::Object(body.clone())).await?;
                if status.is_success() {
                    return parse_response(&text);
                }
                let retryable =
                    status.as_u16() == 429 || status.as_u16() == 408 || status.is_server_error();
                if retryable && attempt < self.max_retries {
                    let delay = Duration::from_millis(250 * 2_u64.pow(attempt.min(3)));
                    tokio::time::sleep(delay).await;
                    attempt += 1;
                    continue;
                }
                let detail = api_error(&text);
                return Err(LlmError::Api(format!("HTTP {}: {detail}", status.as_u16())));
            }
        })
    }
}

fn message_json(message: &ChatMessage) -> Value {
    let mut out = Map::new();
    out.insert("role".into(), json!(message.role));
    match message.role.as_str() {
        "assistant" => {
            out.insert(
                "content".into(),
                message.content.clone().map_or(Value::Null, Value::String),
            );
            if !message.tool_calls.is_empty() {
                out.insert(
                    "tool_calls".into(),
                    Value::Array(
                        message
                            .tool_calls
                            .iter()
                            .map(|call| {
                                json!({
                                    "id": call.id,
                                    "type": "function",
                                    "function": {
                                        "name": call.name,
                                        "arguments": call.arguments_raw,
                                    }
                                })
                            })
                            .collect(),
                    ),
                );
            }
        }
        "tool" => {
            out.insert("content".into(), json!(message.content));
            out.insert("tool_call_id".into(), json!(message.tool_call_id));
        }
        _ => {
            out.insert("content".into(), json!(message.content));
        }
    }
    Value::Object(out)
}

#[derive(Debug, Deserialize)]
struct Completion {
    choices: Vec<Choice>,
}

#[derive(Debug, Deserialize)]
struct Choice {
    message: ResponseMessage,
}

#[derive(Debug, Deserialize)]
struct ResponseMessage {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<ResponseToolCall>,
}

#[derive(Debug, Deserialize)]
struct ResponseToolCall {
    #[serde(default)]
    id: Option<String>,
    function: FunctionCall,
}

#[derive(Debug, Deserialize)]
struct FunctionCall {
    name: String,
    #[serde(default)]
    arguments: String,
}

fn parse_response(text: &str) -> Result<ChatResponse, LlmError> {
    let completion: Completion = serde_json::from_str(text)
        .map_err(|e| LlmError::Protocol(format!("cannot parse chat completion: {e}")))?;
    let choice = completion
        .choices
        .into_iter()
        .next()
        .ok_or_else(|| LlmError::Protocol("chat completion had no choices".into()))?;
    let content = choice
        .message
        .content
        .filter(|text| !text.trim().is_empty());
    let tool_calls: Vec<ToolCall> = choice
        .message
        .tool_calls
        .into_iter()
        .enumerate()
        .map(|(index, call)| ToolCall {
            id: call
                .id
                .filter(|id| !id.trim().is_empty())
                .unwrap_or_else(|| format!("call-{index}")),
            name: call.function.name,
            arguments_raw: if call.function.arguments.trim().is_empty() {
                "{}".into()
            } else {
                call.function.arguments
            },
        })
        .collect();
    if content.is_none() && tool_calls.is_empty() {
        return Err(LlmError::Protocol(
            "chat completion had neither content nor tool calls".into(),
        ));
    }
    Ok(ChatResponse {
        content,
        tool_calls,
    })
}

fn api_error(text: &str) -> String {
    let parsed: Option<Value> = serde_json::from_str(text).ok();
    parsed
        .as_ref()
        .and_then(|v| v.get("error"))
        .and_then(|e| e.get("message"))
        .and_then(Value::as_str)
        .map(|message| truncate(message, 400))
        .unwrap_or_else(|| truncate(text, 400))
}

fn sanitize_error(message: &str) -> String {
    // Transport errors can include the URL, never the bearer token; cap length anyway.
    truncate(message, 400)
}

fn truncate(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        value.to_string()
    } else {
        let mut out: String = value.chars().take(max).collect();
        out.push('…');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(base_url: String) -> LlmConfig {
        LlmConfig {
            api_key: "test-key-not-real".into(),
            base_url,
            model: "test-model".into(),
            temperature: 0.0,
            timeout: Duration::from_secs(5),
            max_retries: 0,
        }
    }

    #[test]
    fn parses_tool_calls_and_generates_missing_ids() {
        let body = r#"{"choices":[{"message":{"content":null,"tool_calls":[
            {"function":{"name":"plan_priorities","arguments":"{\"days\":7}"}},
            {"id":"call_2","function":{"name":"refuse_request","arguments":"{\"reason\":\"out_of_scope\"}"}}
        ]}}]}"#;
        let response = parse_response(body).unwrap();
        assert_eq!(response.tool_calls.len(), 2);
        assert_eq!(response.tool_calls[0].id, "call-0");
        assert_eq!(
            response.tool_calls[0].arguments().unwrap(),
            json!({"days": 7})
        );
        assert_eq!(response.tool_calls[1].id, "call_2");
    }

    #[test]
    fn rejects_empty_completions() {
        assert!(parse_response(r#"{"choices":[{"message":{}}]}"#).is_err());
        assert!(parse_response(r#"{"choices":[]}"#).is_err());
    }

    #[test]
    fn api_errors_are_capped_and_readable() {
        let message = api_error(r#"{"error":{"message":"bad request"}}"#);
        assert_eq!(message, "bad request");
        assert!(api_error(&"x".repeat(1000)).chars().count() <= 401);
    }

    #[tokio::test]
    async fn sends_tools_and_parses_a_mocked_completion_without_a_real_key() {
        use wiremock::matchers::{header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .and(header("authorization", "Bearer test-key-not-real"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "choices": [{"message": {"content": "Start with [S1].", "tool_calls": []}}]
            })))
            .mount(&server)
            .await;

        let client = OpenAiClient::new(&config(format!("{}/v1", server.uri()))).unwrap();
        let tools = vec![json!({
            "type": "function",
            "function": {"name": "plan_priorities", "description": "plan", "parameters": {"type": "object"}}
        })];
        let response = client
            .chat(&[ChatMessage::user("What should I do?")], &tools)
            .await
            .unwrap();
        assert_eq!(response.content.as_deref(), Some("Start with [S1]."));
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        let body: Value = serde_json::from_slice(&requests[0].body).unwrap();
        assert_eq!(body["tools"][0]["function"]["name"], "plan_priorities");
        assert_eq!(body["model"], "test-model");
    }
}
