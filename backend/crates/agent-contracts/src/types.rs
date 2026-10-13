use serde::{Deserialize, Serialize};

pub type BackendId = String;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContentBlock {
    Text {
        text: String,
    },
    Image {
        mime_type: String,
        data_base64: String,
    },
    Audio {
        mime_type: String,
        data_base64: String,
    },
    Resource {
        uri: String,
        mime_type: Option<String>,
    },
}

impl ContentBlock {
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

impl BackendError {
    pub fn new(code: impl Into<String>, message: impl Into<String>, retryable: bool) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            retryable,
        }
    }

    pub fn unsupported(message: impl Into<String>) -> Self {
        Self::new("unsupported", message, false)
    }

    pub fn protocol(message: impl Into<String>) -> Self {
        Self::new("protocol", message, false)
    }

    pub fn unavailable(message: impl Into<String>) -> Self {
        Self::new("unavailable", message, true)
    }

    pub fn interrupted(message: impl Into<String>) -> Self {
        Self::new("interrupted", message, true)
    }
}

impl std::fmt::Display for BackendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

/// Authoritative terminal result of one backend run.
///
/// A message stream closing cleanly does **not** imply success. Callers must
/// use this value, not event-stream completion, to decide operation status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum BackendOutcome {
    Completed { content: Vec<ContentBlock> },
    Failed { error: BackendError },
    Cancelled,
    Interrupted { error: BackendError },
}

impl BackendOutcome {
    pub fn completed_text(text: impl Into<String>) -> Self {
        Self::Completed {
            content: vec![ContentBlock::text(text)],
        }
    }

    pub fn is_terminal_success(&self) -> bool {
        matches!(self, Self::Completed { .. })
    }

    pub fn text(&self) -> String {
        match self {
            Self::Completed { content } => content
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(""),
            Self::Failed { error } | Self::Interrupted { error } => error.message.clone(),
            Self::Cancelled => String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendRunContext {
    pub chat_session_id: String,
    pub agent_session_id: String,
    pub vault_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunRequest {
    pub run_id: String,
    pub input: Vec<ContentBlock>,
    pub context: Option<BackendRunContext>,
}

impl RunRequest {
    pub fn text(run_id: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            run_id: run_id.into(),
            input: vec![ContentBlock::text(text)],
            context: None,
        }
    }

    pub fn with_context(
        mut self,
        chat_session_id: impl Into<String>,
        agent_session_id: impl Into<String>,
        vault_id: Option<String>,
    ) -> Self {
        self.context = Some(BackendRunContext {
            chat_session_id: chat_session_id.into(),
            agent_session_id: agent_session_id.into(),
            vault_id,
        });
        self
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendCapabilities {
    pub session_resume: bool,
    pub cancel: bool,
    pub permissions: bool,
    pub user_input: bool,
    pub image_input: bool,
    pub audio_input: bool,
    pub resource_input: bool,
    pub mcp: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PermissionDecision {
    AllowOnce,
    Deny,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackendPermissionOption {
    pub id: String,
    pub label: String,
}

/// Provider-neutral permission request. The payload is intentionally opaque
/// JSON: adapters preserve provider details without making the application
/// depend on a provider request type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackendPermissionRequest {
    pub request_id: String,
    pub kind: String,
    pub summary: String,
    pub payload: serde_json::Value,
    pub options: Vec<BackendPermissionOption>,
}

/// Provider-neutral user-input request. A backend may suspend a run until the
/// matching `respond_input` call is accepted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackendInputRequest {
    pub request_id: String,
    pub prompt: String,
    pub secret: bool,
    pub choices: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum BackendEvent {
    MessageDelta {
        text: String,
    },
    Activity {
        description: String,
    },
    ToolCall {
        call_id: String,
        name: String,
        arguments: String,
    },
    ToolResult {
        call_id: String,
        ok: bool,
        summary: String,
    },
    PermissionRequested {
        request: BackendPermissionRequest,
    },
    InputRequested {
        request: BackendInputRequest,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_outcome_is_not_success() {
        let outcome = BackendOutcome::Failed {
            error: BackendError::protocol("max_tokens"),
        };
        assert!(!outcome.is_terminal_success());
        assert_eq!(outcome.text(), "max_tokens");
    }

    #[test]
    fn completed_text_joins_content_blocks() {
        let outcome = BackendOutcome::Completed {
            content: vec![ContentBlock::text("a"), ContentBlock::text("b")],
        };
        assert!(outcome.is_terminal_success());
        assert_eq!(outcome.text(), "ab");
    }

    #[test]
    fn backend_error_constructors() {
        assert!(!BackendError::unsupported("x").retryable);
        assert!(BackendError::interrupted("x").retryable);
        assert_eq!(BackendError::unavailable("y").code, "unavailable");
    }

    #[test]
    fn run_request_context_is_neutral_and_serializable() {
        let request = RunRequest::text("run-1", "hello").with_context(
            "chat-1",
            "agent-1",
            Some("vault-1".to_owned()),
        );
        assert_eq!(
            request.context,
            Some(BackendRunContext {
                chat_session_id: "chat-1".to_owned(),
                agent_session_id: "agent-1".to_owned(),
                vault_id: Some("vault-1".to_owned()),
            })
        );
        let encoded = serde_json::to_value(&request).expect("request json");
        assert_eq!(encoded["context"]["chat_session_id"], "chat-1");
        assert_eq!(encoded["context"]["vault_id"], "vault-1");
    }
}
