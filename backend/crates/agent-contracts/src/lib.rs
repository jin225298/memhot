//! Protocol-neutral agent backend contracts.
//!
//! No dependency on Tauri, SQLite, Hermes, reminder business, or any concrete
//! provider. Host composition roots implement [`crate::session::BackendSession`].

pub mod conversation;
pub mod invocation;
pub mod session;
pub mod types;

pub use conversation::{
    ConversationEvent, ConversationEventAdapter, ToolCall, ToolDefinition, ToolError, ToolHandler,
    ToolRegistry, ToolResult,
};
pub use invocation::{CallKind, CallStatus, ModelCall, UsageRecord, VoiceCall};
pub use session::{BackendEventSink, BackendSession};
pub use types::{
    BackendCapabilities, BackendError, BackendEvent, BackendId, BackendInputRequest,
    BackendOutcome, BackendPermissionOption, BackendPermissionRequest, BackendRunContext,
    ContentBlock, PermissionDecision, RunRequest,
};
