//! 真实第二 provider 冒烟（默认 ignored；不进 CI、不消耗额度）。
//!
//! 运行方式（显式指定真实 OpenAI 兼容服务端）：
//!
//! ```sh
//! SHIKE_SMOKE_MODEL_ENDPOINT=https://<host>/v1 \
//! SHIKE_SMOKE_MODEL_KEY=sk-... \
//! SHIKE_SMOKE_MODEL_NAME=<model-id> \
//! cargo test -p model-runtime --test live_smoke -- --ignored --nocapture
//! ```
//!
//! 缺少任一环境变量时给出明确跳过原因（不视为失败）。

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use model_runtime::catalog::{CatalogError, ModelConfigSource, ModelTarget};
use model_runtime::contracts::{
    ChatTurn, ModelCall, ModelOutcome, ModelRequest, OutputSpec, TextModelPort,
};
use model_runtime::policy::{self, workload};
use model_runtime::ModelRuntime;
use provider_config::ProviderCapability;

struct SingleTarget {
    endpoint: String,
    model: String,
    key: String,
}

impl ModelConfigSource for SingleTarget {
    fn list_targets(&self) -> Result<Vec<ModelTarget>, CatalogError> {
        Ok(vec![ModelTarget {
            id: "smoke".to_owned(),
            provider: "smoke".to_owned(),
            endpoint: self.endpoint.clone(),
            model: self.model.clone(),
            revision: 1,
            capabilities: BTreeSet::from([
                ProviderCapability::ChatCompletion,
                ProviderCapability::StructuredOutput,
            ]),
        }])
    }

    fn credential(&self, _target_id: &str) -> Result<Option<String>, CatalogError> {
        Ok(Some(self.key.clone()))
    }
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

#[tokio::test]
#[ignore = "需要真实模型服务端与额度；显式设置 SHIKE_SMOKE_MODEL_* 后运行"]
async fn live_second_provider_completes_structured_and_text_workloads() {
    let (Some(endpoint), Some(key), Some(model)) = (
        env("SHIKE_SMOKE_MODEL_ENDPOINT"),
        env("SHIKE_SMOKE_MODEL_KEY"),
        env("SHIKE_SMOKE_MODEL_NAME"),
    ) else {
        eprintln!("SKIP: 未设置 SHIKE_SMOKE_MODEL_ENDPOINT/_KEY/_NAME，真实冒烟未执行");
        return;
    };

    let source = Arc::new(SingleTarget {
        endpoint,
        model,
        key,
    });
    // smoke 用独立 policy：单候选单 attempt，默认 deadline。
    let policies = policy::builtin_defaults("smoke");
    let port: Arc<dyn TextModelPort> = Arc::new(ModelRuntime::new(source, policies));

    // 1. 结构化 workload（json_object 硬要求）。
    let structured = port
        .complete(ModelCall {
            call_id: "smoke-structured".to_owned(),
            workload: workload::ORGANIZER_STRUCTURED.to_owned(),
            request: ModelRequest {
                messages: vec![
                    ChatTurn::system("只输出一个合法 JSON 对象。"),
                    ChatTurn::user("输出 {\"status\":\"ok\"}"),
                ],
                output: OutputSpec::Json,
                max_tokens: Some(256),
                temperature: None,
            },
            deadline: Duration::from_secs(60),
            cancel: Default::default(),
        })
        .await;
    match &structured.outcome {
        ModelOutcome::Completed {
            content,
            finish_reason,
            usage,
        } => {
            eprintln!("structured ok: finish={finish_reason} usage={usage:?} content={content}");
            assert!(
                serde_json::from_str::<serde_json::Value>(content).is_ok(),
                "structured workload 必须返回合法 JSON，实际：{content}"
            );
        }
        ModelOutcome::Failed { error } => panic!("structured smoke failed: {error:?}"),
    }

    // 2. 纯文本 workload。
    let text = port
        .complete(ModelCall {
            call_id: "smoke-minutes".to_owned(),
            workload: workload::MINUTES_SUMMARIZE.to_owned(),
            request: ModelRequest {
                messages: vec![
                    ChatTurn::system("用一句话回答。"),
                    ChatTurn::user("1+1 等于几？"),
                ],
                output: OutputSpec::PlainText,
                max_tokens: None,
                temperature: Some(0.3),
            },
            deadline: Duration::from_secs(60),
            cancel: Default::default(),
        })
        .await;
    match &text.outcome {
        ModelOutcome::Completed { content, .. } => {
            eprintln!("text ok: {content}");
            assert!(!content.trim().is_empty());
        }
        ModelOutcome::Failed { error } => panic!("text smoke failed: {error:?}"),
    }

    for result in [structured, text] {
        for attempt in &result.attempts {
            eprintln!(
                "attempt {} target={} model={:?} status={:?} latency={}ms",
                attempt.attempt_id,
                attempt.target_id,
                attempt.returned_model,
                attempt.status,
                attempt.latency_ms
            );
        }
    }
}
