//! 统一执行：attempt 循环、重试/替代/预算/取消的唯一 owner。
//!
//! 算法（计划 §3.4）：
//! 1. cancel → Failed{Cancelled}；未知 workload → Failed{Unavailable}。
//! 2. 冻结候选快照（一次 list_targets）；缺能力候选记 Skipped。
//! 3. attempt 循环：per-target 对 retryable 错误最多连试 2 次；
//!    非 retryable 立即换候选（仅 allow_fallback）；max_attempts 与
//!    deadline 全局封顶；退避尊重 Retry-After 并响应 cancel/deadline。
//! 4. 所有候选耗尽 → 返回最后一次真实错误，attempts 全记录。

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::RwLock;

use crate::catalog::{self, ModelConfigSource, ModelTarget};
use crate::contracts::{
    AttemptRecord, AttemptStatus, CallKind, CallStatus, CancelFlag, ModelCall, ModelCallResult,
    ModelError, ModelErrorCode, ModelOutcome, ModelTokenSink, TextModelPort, UsageRecord,
};
use crate::cost::CostLedger;
use crate::policy::PolicySet;

const PER_TARGET_ATTEMPTS: u32 = 2;
const BACKOFF_BASE_MS: u64 = 500;
const BACKOFF_CAP_MS: u64 = 8_000;

pub struct ModelRuntime {
    source: Arc<dyn ModelConfigSource>,
    policies: RwLock<PolicySet>,
    http: reqwest::Client,
    cost_ledger: Option<Arc<CostLedger>>,
}

impl ModelRuntime {
    pub fn new(source: Arc<dyn ModelConfigSource>, policies: PolicySet) -> Self {
        Self::with_http_client(
            source,
            policies,
            reqwest::Client::builder()
                .user_agent("shike-model-runtime/0.1")
                .build()
                .expect("reqwest client"),
        )
    }

    /// 注入 http client（测试用 no_proxy client 保证 127.0.0.1 mock
    /// 不被环境代理劫持；生产默认走 env 代理设置）。
    pub fn with_http_client(
        source: Arc<dyn ModelConfigSource>,
        policies: PolicySet,
        http: reqwest::Client,
    ) -> Self {
        Self {
            source,
            policies: RwLock::new(policies),
            http,
            cost_ledger: None,
        }
    }

    /// 配置重载入口（host 在 model_workloads.json 变化时调用）。
    pub async fn replace_policies(&self, policies: PolicySet) {
        *self.policies.write().await = policies;
    }

    /// Attach an optional bounded/durable cost ledger. Keeping this opt-in
    /// preserves existing callers while making accounting a runtime concern,
    /// not a business-module responsibility.
    pub fn with_cost_ledger(mut self, ledger: Arc<CostLedger>) -> Self {
        self.cost_ledger = Some(ledger);
        self
    }

    /// 只读查询某 workload 的生效策略（诊断/测试用）。
    pub async fn workload_policy(&self, workload: &str) -> Option<crate::policy::WorkloadPolicy> {
        self.policies.read().await.resolve(workload).ok().cloned()
    }

    pub async fn execute(&self, call: ModelCall) -> ModelCallResult {
        self.execute_with_sink(call, None).await
    }

    /// Execute a call through the same retry/budget machinery as `execute`,
    /// forwarding native provider deltas to `sink` when the target supports
    /// Chat Completions SSE. The final `ModelCallResult` remains authoritative
    /// for structured-output validation and usage accounting.
    pub async fn stream(&self, call: ModelCall, sink: ModelTokenSink) -> ModelCallResult {
        self.execute_with_sink(call, Some(sink)).await
    }

    async fn execute_with_sink(
        &self,
        call: ModelCall,
        sink: Option<ModelTokenSink>,
    ) -> ModelCallResult {
        let result = self.execute_inner(call.clone(), sink).await;
        if let Some(ledger) = &self.cost_ledger {
            if let Ok(targets) = self.source.list_targets() {
                if let Err(error) =
                    ledger.record_attempts(&call.call_id, &result.attempts, &targets)
                {
                    tracing::warn!(call_id = %call.call_id, %error, "cost ledger write failed");
                }
                let observed_at_ms = now_ms();
                for attempt in &result.attempts {
                    let target = targets.iter().find(|target| target.id == attempt.target_id);
                    let status = match attempt.status {
                        AttemptStatus::Completed => CallStatus::Completed,
                        AttemptStatus::Failed { .. } => CallStatus::Failed,
                        AttemptStatus::Skipped { .. } => CallStatus::Interrupted,
                    };
                    ledger.record_usage(UsageRecord {
                        call_id: call.call_id.clone(),
                        kind: match call.request.output {
                            crate::contracts::OutputSpec::Json => CallKind::Structured,
                            crate::contracts::OutputSpec::PlainText => CallKind::Text,
                        },
                        provider: target.map(|target| target.provider.clone()),
                        model: target.map(|target| target.model.clone()),
                        prompt_tokens: attempt.usage.and_then(|usage| usage.prompt_tokens),
                        completion_tokens: attempt.usage.and_then(|usage| usage.completion_tokens),
                        input_audio_ms: None,
                        output_audio_ms: None,
                        request_id: attempt.provider_request_id.clone(),
                        status,
                        observed_at_ms,
                        cost_micros_usd: None,
                    });
                }
            }
        }
        result
    }

    async fn execute_inner(
        &self,
        call: ModelCall,
        sink: Option<ModelTokenSink>,
    ) -> ModelCallResult {
        if call.cancel.is_cancelled() {
            return cancelled_result();
        }
        let policy = {
            let policies = self.policies.read().await;
            match policies.resolve(&call.workload) {
                Ok(policy) => policy.clone(),
                Err(error) => {
                    return failed_result(
                        ModelError::new(ModelErrorCode::Unavailable, error.to_string(), false),
                        Vec::new(),
                    )
                }
            }
        };

        // 冻结候选快照。
        let targets = match self.source.list_targets() {
            Ok(targets) => targets,
            Err(error) => {
                return failed_result(
                    ModelError::new(
                        ModelErrorCode::Unavailable,
                        format!("config source: {error}"),
                        false,
                    ),
                    Vec::new(),
                )
            }
        };
        let frozen = catalog::resolve_candidates(&targets, &policy.candidates, &policy.requires);
        let has_callable = frozen.iter().any(|(_, skip)| skip.is_none());
        if !has_callable {
            let reason = frozen
                .iter()
                .map(|(target, skip)| match skip {
                    Some(reason) => reason.clone(),
                    None => format!("candidate `{}` usable", target.id),
                })
                .collect::<Vec<_>>()
                .join("; ");
            let mut attempts = Vec::new();
            for (target, skip) in &frozen {
                if let Some(reason) = skip {
                    attempts.push(skipped_record(&call.call_id, target, reason.clone()));
                }
            }
            return failed_result(
                ModelError::new(
                    ModelErrorCode::Unavailable,
                    format!(
                        "no usable target for workload `{}`: {reason}",
                        call.workload
                    ),
                    false,
                ),
                attempts,
            );
        }

        // deadline 语义：调用方显式给定（≠ MAX）时完全覆盖 workload 兜底
        // （organizer 的 plan.timeout_seconds 可大于内置 90s，旧路径行为）；
        // 未给定（MAX，PortChatCompleter/minutes 等业务缝）时用 policy 兜底。
        let budget = std::time::Instant::now();
        let deadline = if call.deadline == Duration::MAX {
            policy.deadline()
        } else {
            call.deadline
        };
        let mut attempts: Vec<AttemptRecord> = Vec::new();
        let mut attempt_no: u32 = 0;
        let mut last_error: Option<ModelError> = None;
        let mut candidate_index = 0usize;

        while attempt_no < policy.max_attempts {
            if call.cancel.is_cancelled() {
                return failed_result(
                    ModelError::new(ModelErrorCode::Cancelled, "cancelled", false),
                    attempts,
                );
            }
            let elapsed = budget.elapsed();
            if elapsed >= deadline {
                let error = last_error.unwrap_or_else(|| {
                    ModelError::new(
                        ModelErrorCode::BudgetExhausted,
                        format!("deadline {deadline:?} exhausted before next attempt"),
                        false,
                    )
                });
                return failed_result(error, attempts);
            }

            let (target, skip_reason) = &frozen[candidate_index];
            if let Some(reason) = skip_reason {
                attempts.push(skipped_record(&call.call_id, target, reason.clone()));
                // 推进候选并继续（Skipped 不消耗 max_attempts——它没有产生
                // 网络调用；但防止死循环：没有更多候选时退出）。
                if candidate_index + 1 < frozen.len() {
                    candidate_index += 1;
                    continue;
                }
                let error = last_error.unwrap_or_else(|| {
                    ModelError::new(
                        ModelErrorCode::Unavailable,
                        "all remaining candidates skipped",
                        false,
                    )
                });
                return failed_result(error, attempts);
            }

            let remaining = deadline - elapsed;
            let started = std::time::Instant::now();
            let credential = self.source.credential(&target.id).ok().flatten();
            attempt_no += 1;
            let result = match credential {
                Some(key) => match sink.as_ref() {
                    Some(sink) => {
                        crate::adapters::chat_completions::invoke_stream_for_workload(
                            &self.http,
                            target,
                            &key,
                            &call.request,
                            remaining,
                            sink.clone(),
                            &call.workload,
                        )
                        .await
                    }
                    None => {
                        crate::adapters::chat_completions::invoke_for_workload(
                            &self.http,
                            target,
                            &key,
                            &call.request,
                            remaining,
                            &call.workload,
                        )
                        .await
                    }
                },
                None => Err(ModelError::new(
                    ModelErrorCode::Auth,
                    format!("target `{}` has no credential configured", target.id),
                    false,
                )),
            };
            let latency = started.elapsed();

            match result {
                Ok(success) => {
                    let record = AttemptRecord {
                        attempt_id: format!("{}#{}", call.call_id, attempt_no),
                        target_id: target.id.clone(),
                        target_revision: target.revision,
                        requested_model: target.model.clone(),
                        returned_model: success.returned_model.clone(),
                        provider_request_id: success.provider_request_id.clone(),
                        status: AttemptStatus::Completed,
                        error_message: None,
                        usage: success.usage,
                        latency_ms: latency.as_millis() as u64,
                    };
                    tracing::info!(
                        workload = %call.workload,
                        target = %target.id,
                        attempt = attempt_no,
                        latency_ms = record.latency_ms,
                        usage = ?record.usage,
                        "model call completed"
                    );
                    attempts.push(record);
                    return ModelCallResult {
                        outcome: ModelOutcome::Completed {
                            content: success.content,
                            finish_reason: success.finish_reason,
                            usage: success.usage,
                        },
                        attempts,
                    };
                }
                Err(error) => {
                    tracing::warn!(
                        workload = %call.workload,
                        target = %target.id,
                        attempt = attempt_no,
                        code = ?error.code,
                        latency_ms = latency.as_millis() as u64,
                        "model call attempt failed"
                    );
                    attempts.push(AttemptRecord {
                        attempt_id: format!("{}#{}", call.call_id, attempt_no),
                        target_id: target.id.clone(),
                        target_revision: target.revision,
                        requested_model: target.model.clone(),
                        returned_model: None,
                        provider_request_id: error.provider_request_id.clone(),
                        status: AttemptStatus::Failed { code: error.code },
                        error_message: Some(error.message.clone()),
                        usage: None,
                        latency_ms: latency.as_millis() as u64,
                    });
                    last_error = Some(error.clone());

                    // 预算尽：返回最后错误。
                    if attempt_no >= policy.max_attempts {
                        return failed_result(error, attempts);
                    }

                    if error.retryable {
                        // 同 target 还剩额度 → 退避后重试；否则换候选。
                        let used_by_target = attempts
                            .iter()
                            .rev()
                            .take_while(|a| a.target_id == target.id)
                            .count() as u32;
                        if used_by_target < PER_TARGET_ATTEMPTS {
                            if let Err(stop) = backoff(&call, &error, deadline, budget).await {
                                return failed_result(stop, attempts);
                            }
                            continue;
                        }
                    }

                    // 换候选（fallback 关闭 → 以当前错误失败）。
                    if !policy.allow_fallback || candidate_index + 1 >= frozen.len() {
                        return failed_result(error, attempts);
                    }
                    candidate_index += 1;
                }
            }
        }

        let error = last_error.unwrap_or_else(|| {
            ModelError::new(
                ModelErrorCode::BudgetExhausted,
                "no attempt executed",
                false,
            )
        });
        failed_result(error, attempts)
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis() as i64)
}

/// 退避等待：尊重 Retry-After；响应 cancel 与 deadline。
async fn backoff(
    call: &ModelCall,
    error: &ModelError,
    deadline: Duration,
    budget: std::time::Instant,
) -> Result<(), ModelError> {
    let requested = error
        .retry_after_ms
        .map(Duration::from_millis)
        .unwrap_or_else(|| Duration::from_millis(BACKOFF_BASE_MS.min(BACKOFF_CAP_MS)));
    let remaining = deadline.saturating_sub(budget.elapsed());
    if remaining.is_zero() {
        return Err(budget_exhausted_after(error));
    }
    let wait = requested.min(remaining);
    tracing::info!(workload = %call.workload, wait_ms = wait.as_millis() as u64, "backoff before retry");
    tokio::select! {
        _ = tokio::time::sleep(wait) => Ok(()),
        _ = tokio::time::sleep(remaining) => Err(budget_exhausted_after(error)),
        _ = await_cancel(call.cancel.clone()) => Err(ModelError::new(
            ModelErrorCode::Cancelled,
            "cancelled during backoff",
            false,
        )),
    }
}

/// 退避期内预算耗尽：保留根因 code，message 注明预算耗尽。
fn budget_exhausted_after(error: &ModelError) -> ModelError {
    ModelError {
        code: error.code,
        message: format!("{}; deadline exhausted during backoff", error.message),
        retryable: false,
        retry_after_ms: None,
        provider_request_id: error.provider_request_id.clone(),
    }
}

async fn await_cancel(flag: CancelFlag) {
    loop {
        if flag.is_cancelled() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

fn cancelled_result() -> ModelCallResult {
    failed_result(
        ModelError::new(ModelErrorCode::Cancelled, "cancelled before start", false),
        Vec::new(),
    )
}

fn failed_result(error: ModelError, attempts: Vec<AttemptRecord>) -> ModelCallResult {
    ModelCallResult {
        outcome: ModelOutcome::Failed { error },
        attempts,
    }
}

fn skipped_record(call_id: &str, target: &ModelTarget, reason: String) -> AttemptRecord {
    tracing::warn!(target = %target.id, %reason, "candidate skipped");
    AttemptRecord {
        attempt_id: format!("{call_id}#skip-{}", target.id),
        target_id: target.id.clone(),
        target_revision: target.revision,
        requested_model: target.model.clone(),
        returned_model: None,
        provider_request_id: None,
        status: AttemptStatus::Skipped { reason },
        error_message: None,
        usage: None,
        latency_ms: 0,
    }
}

#[async_trait::async_trait]
impl TextModelPort for ModelRuntime {
    async fn complete(&self, call: ModelCall) -> ModelCallResult {
        self.execute(call).await
    }

    async fn stream(&self, call: ModelCall, sink: ModelTokenSink) -> ModelCallResult {
        ModelRuntime::stream(self, call, sink).await
    }
}
