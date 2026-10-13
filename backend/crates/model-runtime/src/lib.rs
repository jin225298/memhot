//! Model Runtime：内部文本与结构化模型调用的统一端口。
//!
//! 阶段 1B（见 docs/superpowers/plans/2026-09-14-model-runtime-phase1b.md）：
//! 中立契约（contracts）、target catalog、workload policy、统一执行
//! （execution）与 Chat Completions adapter。业务 prompt、schema 与来源
//! 校验留在各业务模块；重试/替代/预算/取消只有 execution 一个 owner。
//!
//! 本 crate 不依赖 voiceagent-*、task-runtime、agent-*、Tauri、SQLite。

pub mod adapters;
pub mod bootstrap;
pub mod catalog;
pub mod contracts;
pub mod cost;
pub mod execution;
pub mod policy;

pub use adapters::chat_completions;
pub use bootstrap::single_target_runtime;
pub use catalog::{CatalogError, ModelConfigSource, ModelTarget};
pub use contracts::{
    AttemptRecord, AttemptStatus, CallKind, CallStatus, CancelFlag, ChatTurn, ModelCall,
    ModelCallResult, ModelError, ModelErrorCode, ModelOutcome, ModelRequest, ModelTokenSink,
    OutputSpec, TextModelPort, Usage, UsageRecord, VoiceCall, WorkloadId,
};
pub use cost::{CostLedger, CostLedgerError, CostRecord, CostState, ModelPrice, PriceTable};
pub use execution::ModelRuntime;
pub use policy::{PolicyError, PolicySet, WorkloadPolicy};
