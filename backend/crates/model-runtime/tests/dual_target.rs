//! 双 target 同工作负载一致性（Task 6）：两个 OpenAI 兼容服务端（不同
//! endpoint/model、不同响应 envelope 形状）跑同一 workload；fallback 不
//! 丢失 workload 硬要求（json_object + max_tokens）。

mod support;

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use model_runtime::catalog::ModelTarget;
use model_runtime::contracts::{
    ChatTurn, ModelCall, ModelOutcome, ModelRequest, OutputSpec, TextModelPort,
};
use model_runtime::policy::{self, workload, PolicySet, WorkloadPolicy};
use model_runtime::ModelRuntime;
use provider_config::ProviderCapability;
use support::{MockResponse, MockServer, StaticSource};

fn deepseek(endpoint: String) -> ModelTarget {
    ModelTarget {
        id: "deepseek".to_owned(),
        provider: "deepseek".to_owned(),
        endpoint,
        model: "deepseek-v4-flash".to_owned(),
        revision: 4,
        capabilities: BTreeSet::from([
            ProviderCapability::ChatCompletion,
            ProviderCapability::StructuredOutput,
        ]),
    }
}

/// OpenAI 官方风格：response 多 system_fingerprint 字段、无 usage、
/// 无 request-id header——不同协议形状的替代候选。
fn openai_style(endpoint: String) -> ModelTarget {
    ModelTarget {
        id: "pixel".to_owned(),
        provider: "pixel".to_owned(),
        endpoint,
        model: "gpt-5-mini".to_owned(),
        revision: 2,
        ..deepseek(String::new())
    }
}

fn organizer_call() -> ModelCall {
    ModelCall {
        call_id: "dual-1".to_owned(),
        workload: workload::ORGANIZER_STRUCTURED.to_owned(),
        request: ModelRequest {
            messages: vec![ChatTurn::system("sys"), ChatTurn::user("usr")],
            output: OutputSpec::Json,
            max_tokens: Some(8_192),
            temperature: None,
        },
        deadline: Duration::from_secs(10),
        cancel: Default::default(),
    }
}

fn dual_runtime(primary: &MockServer, secondary: &MockServer, fallback: bool) -> Arc<ModelRuntime> {
    let mut policies: PolicySet = policy::builtin_defaults("deepseek");
    let mut base = policies
        .resolve(workload::ORGANIZER_STRUCTURED)
        .unwrap()
        .clone();
    base.candidates = vec!["deepseek".to_owned(), "pixel".to_owned()];
    base.allow_fallback = fallback;
    base.max_attempts = 3;
    policies.upsert(base).unwrap();
    Arc::new(ModelRuntime::with_http_client(
        Arc::new(StaticSource::new(
            vec![
                deepseek(primary.endpoint()),
                openai_style(secondary.endpoint()),
            ],
            vec![
                ("deepseek".to_owned(), "sk-a".to_owned()),
                ("pixel".to_owned(), "sk-b".to_owned()),
            ],
        )),
        policies,
        reqwest_client_no_proxy(),
    ))
}

fn reqwest_client_no_proxy() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("client")
}

#[tokio::test]
async fn primary_target_completes_with_single_attempt() {
    let primary = MockServer::spawn().await;
    primary.enqueue(MockResponse::json(
        200,
        serde_json::json!({
            "choices": [{"message": {"content": "{\"ok\":true}"}, "finish_reason": "stop"}],
            "model": "deepseek-v4-flash",
            "usage": {"prompt_tokens": 3, "completion_tokens": 4}
        })
        .to_string(),
    ));
    let secondary = MockServer::spawn().await;
    let port = dual_runtime(&primary, &secondary, true);
    let result = port.complete(organizer_call()).await;

    assert!(
        matches!(&result.outcome, ModelOutcome::Completed { content, .. } if content.contains("ok"))
    );
    assert_eq!(result.attempts.len(), 1);
    assert_eq!(result.attempts[0].target_id, "deepseek");
    assert!(secondary.requests().is_empty());
}

#[tokio::test]
async fn fallback_target_preserves_workload_hard_requirements() {
    let primary = MockServer::spawn().await;
    primary.enqueue(MockResponse::json(500, "{\"error\":\"boom\"}"));
    primary.enqueue(MockResponse::json(500, "{\"error\":\"boom2\"}"));
    let secondary = MockServer::spawn().await;
    // OpenAI 风格响应：多 system_fingerprint、无 usage。
    secondary.enqueue(MockResponse::json(
        200,
        serde_json::json!({
            "choices": [{"message": {"content": "{\"from\":\"pixel\"}"}, "finish_reason": "stop"}],
            "model": "gpt-5-mini-2026-01",
            "system_fingerprint": "fp_abc"
        })
        .to_string(),
    ));
    let port = dual_runtime(&primary, &secondary, true);
    let result = port.complete(organizer_call()).await;

    match &result.outcome {
        ModelOutcome::Completed { content, usage, .. } => {
            assert!(content.contains("pixel"));
            assert!(usage.is_none(), "未知 usage 如实为 None");
        }
        other => panic!("expected completed, got {other:?}"),
    }
    assert_eq!(result.attempts.len(), 3);

    // 硬要求不因 fallback 丢失：两个 target 收到的请求体都带 json_object
    // + max_tokens；deepseek flavor 只作用于 deepseek target。
    let primary_body: serde_json::Value =
        serde_json::from_str(&primary.requests()[0].body).unwrap();
    assert_eq!(primary_body["response_format"]["type"], "json_object");
    assert_eq!(primary_body["max_tokens"], 8192);
    assert_eq!(primary_body["thinking"]["type"], "disabled");

    let secondary_body: serde_json::Value =
        serde_json::from_str(&secondary.requests()[0].body).unwrap();
    assert_eq!(secondary_body["response_format"]["type"], "json_object");
    assert_eq!(secondary_body["max_tokens"], 8192);
    assert!(secondary_body.get("thinking").is_none());
    assert_eq!(secondary_body["model"], "gpt-5-mini");
    assert_eq!(
        result.attempts[2].returned_model.as_deref(),
        Some("gpt-5-mini-2026-01")
    );
}

#[tokio::test]
async fn fallback_disabled_fails_on_primary_error() {
    let primary = MockServer::spawn().await;
    primary.enqueue(MockResponse::json(500, "{\"error\":\"boom\"}"));
    primary.enqueue(MockResponse::json(500, "{\"error\":\"boom2\"}"));
    let secondary = MockServer::spawn().await;
    let port = dual_runtime(&primary, &secondary, false);
    let result = port.complete(organizer_call()).await;

    assert!(matches!(result.outcome, ModelOutcome::Failed { .. }));
    assert!(secondary.requests().is_empty());
}
