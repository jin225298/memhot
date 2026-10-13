//! Provider-neutral conversation and application tool contracts.
//!
//! Realtime/ACP adapters may translate their wire events into
//! [`ConversationEvent`], but application code must only depend on these
//! values.  Tool registration is deliberately capability-oriented: a
//! provider advertises definitions, while the application owns validation,
//! idempotency and result item IDs.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

/// Adapter boundary used by every realtime provider. The application only
/// consumes `ConversationEvent`; the associated provider event stays inside
/// the adapter crate.
pub trait ConversationEventAdapter {
    type ProviderEvent;

    fn map(&self, event: Self::ProviderEvent) -> ConversationEvent;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConversationEvent {
    SessionCreated {
        session_id: Option<String>,
    },
    SessionUpdated {
        session_id: Option<String>,
    },
    ResponseCreated {
        response_id: String,
    },
    ResponseDone {
        response_id: String,
        status: Option<String>,
    },
    ToolCallDelta {
        response_id: String,
        call_id: String,
        name: String,
        delta: String,
    },
    ToolCall {
        response_id: String,
        call_id: String,
        name: String,
        arguments: String,
    },
    AudioDelta {
        response_id: String,
        pcm16: Vec<u8>,
    },
    AudioDone {
        response_id: String,
    },
    TranscriptDelta {
        response_id: String,
        delta: String,
    },
    TranscriptDone {
        response_id: String,
        transcript: String,
    },
    TextDelta {
        response_id: String,
        delta: String,
    },
    TextDone {
        response_id: String,
        text: String,
    },
    InputSpeechStarted {
        item_id: Option<String>,
    },
    InputSpeechStopped {
        item_id: Option<String>,
    },
    ConversationItemCreated {
        item_id: Option<String>,
    },
    InputTranscriptionDelta {
        item_id: Option<String>,
        transcript: String,
    },
    InputTranscriptionCompleted {
        item_id: Option<String>,
        transcript: String,
    },
    Error {
        code: Option<String>,
        kind: Option<String>,
        message: String,
    },
    Unknown {
        event_type: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: Value,
    /// Stable application capability key used for policy/UI exposure.
    pub capability: String,
}

impl ToolDefinition {
    /// Encode the neutral definition for an OpenAI/Qwen-compatible function
    /// list. Other adapters may choose a different envelope.
    pub fn to_function_json(&self) -> Value {
        serde_json::json!({
            "type": "function",
            "name": self.name,
            "description": self.description,
            "parameters": self.parameters,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    pub response_id: Option<String>,
    pub call_id: String,
    pub name: String,
    pub arguments: Value,
    /// Defaults to `call_id`; adapters can provide a durable provider key.
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolResult {
    pub call_id: String,
    /// Stable function-call output item ID. Replays use the same ID.
    pub item_id: String,
    pub ok: bool,
    pub output: Value,
    pub replayed: bool,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ToolError {
    #[error("unknown tool `{0}`")]
    UnknownTool(String),
    #[error("invalid tool call: {0}")]
    InvalidCall(String),
    #[error("tool `{name}` failed: {message}")]
    Handler { name: String, message: String },
    #[error("duplicate tool definition `{0}`")]
    DuplicateDefinition(String),
}

#[async_trait]
pub trait ToolHandler: Send + Sync {
    async fn handle(&self, call: ToolCall) -> Result<Value, ToolError>;
}

struct RegisteredTool {
    definition: ToolDefinition,
    handler: Arc<dyn ToolHandler>,
}

/// Application-owned registry. Provider adapters only consume
/// [`ToolDefinition`] and submit [`ToolCall`]; they never execute handlers or
/// invent result item IDs.
pub struct ToolRegistry {
    tools: BTreeMap<String, RegisteredTool>,
    completed: Mutex<HashMap<String, ToolResult>>,
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: BTreeMap::new(),
            completed: Mutex::new(HashMap::new()),
        }
    }

    pub fn register(
        &mut self,
        definition: ToolDefinition,
        handler: Arc<dyn ToolHandler>,
    ) -> Result<(), ToolError> {
        if self.tools.contains_key(&definition.name) {
            return Err(ToolError::DuplicateDefinition(definition.name));
        }
        if definition.name.trim().is_empty() || definition.capability.trim().is_empty() {
            return Err(ToolError::InvalidCall(
                "tool name and capability must not be blank".to_owned(),
            ));
        }
        if !definition.parameters.is_object() {
            return Err(ToolError::InvalidCall(
                "tool parameters must be a JSON schema object".to_owned(),
            ));
        }
        self.tools.insert(
            definition.name.clone(),
            RegisteredTool {
                definition,
                handler,
            },
        );
        Ok(())
    }

    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools
            .values()
            .map(|tool| tool.definition.clone())
            .collect()
    }

    pub fn definition(&self, name: &str) -> Option<ToolDefinition> {
        self.tools.get(name).map(|tool| tool.definition.clone())
    }

    pub async fn dispatch(&self, call: ToolCall) -> ToolResult {
        let key = call
            .idempotency_key
            .clone()
            .unwrap_or_else(|| call.call_id.clone());
        if let Some(previous) = self
            .completed
            .lock()
            .expect("tool registry completed")
            .get(&key)
            .cloned()
        {
            return ToolResult {
                replayed: true,
                ..previous
            };
        }

        let result = if let Some(tool) = self.tools.get(&call.name) {
            if let Err(error) = validate_call(&tool.definition, &call) {
                Err(error)
            } else {
                tool.handler.handle(call.clone()).await
            }
        } else {
            Err(ToolError::UnknownTool(call.name.clone()))
        };
        let output = match result {
            Ok(output) => ToolResult {
                call_id: call.call_id.clone(),
                item_id: format!("tool-result:{}", call.call_id),
                ok: true,
                output,
                replayed: false,
            },
            Err(error) => ToolResult {
                call_id: call.call_id.clone(),
                item_id: format!("tool-result:{}", call.call_id),
                ok: false,
                output: serde_json::json!({ "error": error.to_string() }),
                replayed: false,
            },
        };
        self.completed
            .lock()
            .expect("tool registry completed")
            .insert(key, output.clone());
        output
    }
}

fn validate_call(definition: &ToolDefinition, call: &ToolCall) -> Result<(), ToolError> {
    let schema = &definition.parameters;
    let object = call.arguments.as_object().ok_or_else(|| {
        ToolError::InvalidCall(format!("tool `{}` arguments must be an object", call.name))
    })?;
    if schema.get("type").and_then(Value::as_str) == Some("object") {
        let required = schema
            .get("required")
            .and_then(Value::as_array)
            .into_iter()
            .flatten();
        for field in required.filter_map(Value::as_str) {
            if !object.contains_key(field) {
                return Err(ToolError::InvalidCall(format!(
                    "tool `{}` is missing `{field}`",
                    call.name
                )));
            }
        }
        if schema.get("additionalProperties").and_then(Value::as_bool) == Some(false) {
            if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
                if let Some(unknown) = object.keys().find(|key| !properties.contains_key(*key)) {
                    return Err(ToolError::InvalidCall(format!(
                        "tool `{}` has unknown argument `{unknown}`",
                        call.name
                    )));
                }
            }
        }
        if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
            for (field, value) in object {
                let Some(property) = properties.get(field) else {
                    continue;
                };
                if let Some(expected) = property.get("type").and_then(Value::as_str) {
                    let valid = match expected {
                        "string" => value.is_string(),
                        "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
                        "number" => value.is_number(),
                        "boolean" => value.is_boolean(),
                        "array" => value.is_array(),
                        "object" => value.is_object(),
                        _ => true,
                    };
                    if !valid {
                        return Err(ToolError::InvalidCall(format!(
                            "tool `{}` argument `{field}` must be {expected}",
                            call.name
                        )));
                    }
                }
                if let Some(enum_values) = property.get("enum").and_then(Value::as_array) {
                    if !enum_values.iter().any(|candidate| candidate == value) {
                        return Err(ToolError::InvalidCall(format!(
                            "tool `{}` argument `{field}` is outside the allowed values",
                            call.name
                        )));
                    }
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Echo;

    #[async_trait]
    impl ToolHandler for Echo {
        async fn handle(&self, call: ToolCall) -> Result<Value, ToolError> {
            Ok(call.arguments)
        }
    }

    struct Failing;

    #[async_trait]
    impl ToolHandler for Failing {
        async fn handle(&self, _call: ToolCall) -> Result<Value, ToolError> {
            Err(ToolError::Handler {
                name: "echo".into(),
                message: "backend unavailable".into(),
            })
        }
    }

    fn registry() -> ToolRegistry {
        let mut registry = ToolRegistry::new();
        registry
            .register(
                ToolDefinition {
                    name: "echo".into(),
                    description: "echo input".into(),
                    parameters: serde_json::json!({
                        "type":"object",
                        "properties":{"text":{"type":"string"}},
                        "required":["text"],
                        "additionalProperties":false
                    }),
                    capability: "conversation.echo".into(),
                },
                Arc::new(Echo),
            )
            .unwrap();
        registry
    }

    fn call(arguments: Value) -> ToolCall {
        ToolCall {
            response_id: Some("response-1".into()),
            call_id: "call-1".into(),
            name: "echo".into(),
            arguments,
            idempotency_key: None,
        }
    }

    #[tokio::test]
    async fn definitions_validate_unknown_and_missing_arguments() {
        let registry = registry();
        assert_eq!(registry.definitions().len(), 1);
        assert!(!registry.dispatch(call(serde_json::json!({}))).await.ok);
        assert!(
            !registry
                .dispatch(call(serde_json::json!({"text": 42})))
                .await
                .ok
        );
        assert!(
            !registry
                .dispatch(ToolCall {
                    name: "unknown".into(),
                    ..call(serde_json::json!({"text":"x"}))
                })
                .await
                .ok
        );
    }

    #[tokio::test]
    async fn duplicate_call_reuses_stable_result_item_id() {
        let registry = registry();
        let first = registry
            .dispatch(call(serde_json::json!({"text":"x"})))
            .await;
        let second = registry
            .dispatch(call(serde_json::json!({"text":"x"})))
            .await;
        assert!(!first.replayed);
        assert!(second.replayed);
        assert_eq!(first.item_id, "tool-result:call-1");
        assert_eq!(first.item_id, second.item_id);
    }

    #[tokio::test]
    async fn handler_error_is_a_replayable_failed_result() {
        let mut registry = ToolRegistry::new();
        registry
            .register(
                ToolDefinition {
                    name: "echo".into(),
                    description: "echo input".into(),
                    parameters: serde_json::json!({"type":"object"}),
                    capability: "conversation.echo".into(),
                },
                Arc::new(Failing),
            )
            .unwrap();
        let first = registry
            .dispatch(call(serde_json::json!({"text":"x"})))
            .await;
        let second = registry
            .dispatch(call(serde_json::json!({"text":"x"})))
            .await;
        assert!(!first.ok);
        assert!(first.output["error"]
            .as_str()
            .unwrap()
            .contains("unavailable"));
        assert!(second.replayed);
        assert_eq!(first.item_id, second.item_id);
    }
}
