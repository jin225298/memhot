//! 中立契约：请求、终态、错误、usage、attempt 记录与 TextModelPort。
//!
//! 这些类型是 v1 的显式有界集合：消息只有纯文本 role/content；无法表达的
//! 供应商特性必须在 adapter 的 flavor 层处理，不能扩展业务请求。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

pub use agent_contracts::{CallKind, CallStatus, UsageRecord, VoiceCall};

/// 工作负载 ID，如 `organizer.structured`。非空性由 policy 注册入口校验。
pub type WorkloadId = String;

/// 单轮消息。v1 仅支持纯文本；多模态输入属于后续阶段。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ChatTurn {
    /// `system` | `user` | `assistant`
    pub role: String,
    pub content: String,
}

impl ChatTurn {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".into(),
            content: content.into(),
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".into(),
            content: content.into(),
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: "assistant".into(),
            content: content.into(),
        }
    }
}

/// 输出形态：JSON（response_format json_object）或纯文本。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputSpec {
    Json,
    PlainText,
}

/// 中立模型请求。服务商特有参数（如 DeepSeek 的 thinking）不在此层。
#[derive(Debug, Clone)]
pub struct ModelRequest {
    pub messages: Vec<ChatTurn>,
    pub output: OutputSpec,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f64>,
}

impl ModelRequest {
    pub fn new(messages: Vec<ChatTurn>, output: OutputSpec) -> Self {
        Self {
            messages,
            output,
            max_tokens: None,
            temperature: None,
        }
    }
}

/// 供应商 usage 透传；字段缺失为 None（未知如实表达，不填零）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelErrorCode {
    /// 401/403：凭证缺失、无效或没有调用权限。
    Auth,
    /// 402 或供应商明确返回余额/配额耗尽；充值或调整套餐后可恢复。
    Billing,
    /// 429：限流；同 target 退避后可重试。
    RateLimited,
    /// 明确的临时 5xx。
    Server,
    /// 连接失败/断流。
    Network,
    /// 超时（deadline 或连接超时）。
    Timeout,
    /// 响应不合法：坏 envelope、缺终态、空 content、无法解析。
    InvalidResponse,
    /// finish_reason=length：截断不是成功。
    Truncated,
    /// finish_reason=content_filter 或内容策略拒绝。
    ContentRejected,
    /// 上下文超限。
    ContextLength,
    /// 协议/参数不被该 target 支持（硬要求不满足）。
    Unsupported,
    /// 调用预算（attempts/deadline）耗尽。
    BudgetExhausted,
    /// 调用方取消。
    Cancelled,
    /// workload 未注册或没有可用 target。
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelError {
    pub code: ModelErrorCode,
    pub message: String,
    /// 同 target 是否值得重试；只指导 execution 层，不是业务重试许可。
    pub retryable: bool,
    /// 来自 Retry-After 的毫秒数（仅 RateLimited 常见）。
    pub retry_after_ms: Option<u64>,
    /// 供应商返回的脱敏请求编号，供日志关联与客服排障。
    pub provider_request_id: Option<String>,
}

impl ModelError {
    pub fn new(code: ModelErrorCode, message: impl Into<String>, retryable: bool) -> Self {
        Self {
            code,
            message: message.into(),
            retryable,
            retry_after_ms: None,
            provider_request_id: None,
        }
    }

    pub fn with_retry_after(mut self, ms: u64) -> Self {
        self.retry_after_ms = Some(ms);
        self
    }

    pub fn with_provider_request_id(mut self, request_id: impl Into<String>) -> Self {
        self.provider_request_id = Some(request_id.into());
        self
    }
}

/// 明确终态：流正常关闭不构成成功，只有这里给出结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelOutcome {
    Completed {
        content: String,
        finish_reason: String,
        usage: Option<Usage>,
    },
    Failed {
        error: ModelError,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttemptStatus {
    Completed,
    Failed {
        code: ModelErrorCode,
    },
    /// 候选被跳过（缺能力/配置不可用），原因必须可读。
    Skipped {
        reason: String,
    },
}

/// 单次 attempt 的记录：不含凭据与消息正文。
#[derive(Debug, Clone)]
pub struct AttemptRecord {
    pub attempt_id: String,
    pub target_id: String,
    pub target_revision: u64,
    pub requested_model: String,
    pub returned_model: Option<String>,
    pub provider_request_id: Option<String>,
    pub status: AttemptStatus,
    pub error_message: Option<String>,
    pub usage: Option<Usage>,
    pub latency_ms: u64,
}

/// 一次逻辑调用。`call_id` 由应用分配；`deadline` 覆盖所有 attempt 与退避。
#[derive(Debug, Clone)]
pub struct ModelCall {
    pub call_id: String,
    pub workload: WorkloadId,
    pub request: ModelRequest,
    pub deadline: Duration,
    pub cancel: CancelFlag,
}

#[derive(Debug, Clone)]
pub struct ModelCallResult {
    pub outcome: ModelOutcome,
    pub attempts: Vec<AttemptRecord>,
}

/// Callback used by [`TextModelPort::stream`] for incremental assistant text.
///
/// The callback is deliberately synchronous and receives owned chunks so a
/// caller can bridge it to an application event channel without borrowing the
/// HTTP response task. Chunks are ordered and concatenate to the final model
/// content. A provider that does not expose streaming may emit one full chunk
/// through the default trait implementation.
pub type ModelTokenSink = Arc<dyn Fn(String) + Send + Sync>;

/// 协作取消信号：覆盖排队、退避与后续 attempt；已送达的请求靠 deadline 兜底。
#[derive(Clone, Default)]
pub struct CancelFlag(Arc<AtomicBool>);

impl CancelFlag {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

impl std::fmt::Debug for CancelFlag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CancelFlag")
            .field("cancelled", &self.is_cancelled())
            .finish()
    }
}

/// 内部文本/结构化模型调用的唯一端口。业务模块依赖此 trait，不依赖具体协议。
#[async_trait::async_trait]
pub trait TextModelPort: Send + Sync {
    async fn complete(&self, call: ModelCall) -> ModelCallResult;

    /// Execute a call while forwarding provider deltas as they arrive.
    ///
    /// This default keeps every existing adapter and test double source
    /// compatible: it delegates to `complete` and emits the completed content
    /// once. Native adapters override it when the target supports SSE.
    async fn stream(&self, call: ModelCall, sink: ModelTokenSink) -> ModelCallResult {
        let result = self.complete(call).await;
        if let ModelOutcome::Completed { content, .. } = &result.outcome {
            sink(content.clone());
        }
        result
    }
}
