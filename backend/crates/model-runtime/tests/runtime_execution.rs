//! execution + chat-completions adapter 故障注入矩阵（Task 2）。
//!
//! 覆盖计划 §Task 2 的全部场景：请求形状、flavor、重试/替代/预算/取消、
//! 终态语义与错误分类。所有用例经 `ModelRuntime::execute` 黑盒驱动。

mod support;

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use model_runtime::catalog::ModelTarget;
use model_runtime::contracts::{
    CancelFlag, ChatTurn, ModelCall, ModelErrorCode, ModelRequest, ModelTokenSink, OutputSpec,
};
use model_runtime::policy::{self, workload, WorkloadPolicy};
use model_runtime::{AttemptStatus, ModelRuntime, TextModelPort};
use provider_config::ProviderCapability;
use support::{MockResponse, MockServer, StaticSource};

fn ok_body(content: &str) -> String {
    serde_json::json!({
        "choices": [{"message": {"content": content}, "finish_reason": "stop"}],
        "usage": {"prompt_tokens": 11, "completion_tokens": 7},
        "model": "deepseek-v4-flash"
    })
    .to_string()
}

fn deepseek_target(endpoint: String) -> ModelTarget {
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

fn pixel_target(endpoint: String) -> ModelTarget {
    ModelTarget {
        id: "pixel".to_owned(),
        provider: "pixel".to_owned(),
        endpoint,
        model: "gpt-5-mini".to_owned(),
        revision: 1,
        ..deepseek_target(String::new())
    }
}

/// mock 测试专用 client：绕过环境代理（http_proxy 会劫持 127.0.0.1）。
fn no_proxy_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent("shike-model-runtime-test/0.1")
        .no_proxy()
        .build()
        .expect("test client")
}

/// 构造 runtime：deepseek 主候选 + 可选 pixel 备选与策略覆盖。
fn runtime(
    primary: &MockServer,
    secondary: Option<&MockServer>,
    policy_patch: impl FnOnce(WorkloadPolicy) -> WorkloadPolicy,
) -> Arc<ModelRuntime> {
    runtime_with_client(primary, secondary, policy_patch, no_proxy_client())
}

fn runtime_with_client(
    primary: &MockServer,
    secondary: Option<&MockServer>,
    policy_patch: impl FnOnce(WorkloadPolicy) -> WorkloadPolicy,
    client: reqwest::Client,
) -> Arc<ModelRuntime> {
    let mut targets = vec![deepseek_target(primary.endpoint())];
    let mut credentials = vec![("deepseek".to_owned(), "sk-deep".to_owned())];
    if let Some(secondary) = secondary {
        targets.push(pixel_target(secondary.endpoint()));
        credentials.push(("pixel".to_owned(), "sk-pix".to_owned()));
    }
    let mut policies = policy::builtin_defaults("deepseek");
    let base = policies
        .resolve(workload::ORGANIZER_STRUCTURED)
        .unwrap()
        .clone();
    policies.upsert(policy_patch(base)).unwrap();
    Arc::new(ModelRuntime::with_http_client(
        Arc::new(StaticSource::new(targets, credentials)),
        policies,
        client,
    ))
}

fn call(deadline_ms: u64) -> ModelCall {
    ModelCall {
        call_id: "call-1".to_owned(),
        workload: workload::ORGANIZER_STRUCTURED.to_owned(),
        request: ModelRequest {
            messages: vec![ChatTurn::system("sys"), ChatTurn::user("usr")],
            output: OutputSpec::Json,
            max_tokens: Some(8192),
            temperature: None,
        },
        deadline: Duration::from_millis(deadline_ms),
        cancel: CancelFlag::new(),
    }
}

fn failed_code(result: &model_runtime::ModelCallResult) -> ModelErrorCode {
    match &result.outcome {
        model_runtime::ModelOutcome::Failed { error } => error.code,
        other => panic!("expected failure, got {other:?}"),
    }
}

#[tokio::test]
async fn official_mimo_user_answers_and_structured_workloads_wire_use_bounded_output() {
    let server = MockServer::spawn().await;
    let mut target = deepseek_target(format!(
        "http://api.xiaomimimo.com:{}/v1",
        server.addr.port()
    ));
    target.model = "mimo-v2.6-flash".into();
    // Resolve the exact production hostname to this test listener. No request
    // leaves the local test server, and no real credential is loaded.
    let client = reqwest::Client::builder()
        .no_proxy()
        .resolve("api.xiaomimimo.com", server.addr)
        .build()
        .unwrap();
    let runtime = ModelRuntime::with_http_client(
        Arc::new(StaticSource::new(
            vec![target],
            vec![("deepseek".into(), "test-key".into())],
        )),
        policy::builtin_defaults("deepseek"),
        client,
    );
    let cases = [
        (workload::MEMORY02_FORMATION, 8192, true, true),
        (workload::MEMORY02_RECALL, 8192, true, true),
        (workload::TRANSCRIPT_CORRECT, 2048, true, true),
        (workload::ORGANIZER_STRUCTURED, 8192, true, true),
        (workload::PROJECT_ANSWER, 2200, true, true),
        (workload::GENERAL_CHAT, 3000, true, false),
    ];
    for (name, max_tokens, _, structured) in cases {
        server.enqueue(MockResponse::json(200, ok_body("{}")));
        let mut request = call(2000);
        request.workload = name.into();
        request.request.max_tokens = Some(max_tokens);
        if !structured { request.request.output = OutputSpec::PlainText; }
        assert!(matches!(
            runtime.execute(request).await.outcome,
            model_runtime::ModelOutcome::Completed { .. }
        ));
    }
    let requests = server.requests();
    assert_eq!(requests.len(), cases.len());
    for (request, (name, max_tokens, bounded_output, structured)) in requests.iter().zip(cases) {
        let body: serde_json::Value = serde_json::from_str(&request.body).unwrap();
        assert_eq!(request.path, "/v1/chat/completions");
        if structured { assert_eq!(body["response_format"]["type"], "json_object"); }
        else { assert!(body.get("response_format").is_none()); }
        assert_eq!(body["stream"], false);
        if bounded_output {
            assert_eq!(body["thinking"]["type"], "disabled", "{name}");
            assert_eq!(body["max_completion_tokens"], max_tokens, "{name}");
            assert!(body.get("max_tokens").is_none());
        } else {
            assert!(body.get("thinking").is_none());
            assert_eq!(body["max_tokens"], max_tokens, "{name}");
            assert!(body.get("max_completion_tokens").is_none());
        }
    }
}

#[tokio::test]
async fn happy_path_returns_completed_with_usage_and_records_attempt() {
    let server = MockServer::spawn().await;
    server.enqueue(
        MockResponse::json(200, ok_body("{\"a\":1}")).with_header("x-request-id", "req-9"),
    );
    let port = runtime(&server, None, |p| p);
    let result = port.complete(call(5_000)).await;

    match &result.outcome {
        model_runtime::ModelOutcome::Completed {
            content,
            finish_reason,
            usage,
        } => {
            assert_eq!(content, "{\"a\":1}");
            assert_eq!(finish_reason, "stop");
            let usage = usage.as_ref().unwrap();
            assert_eq!(usage.prompt_tokens, Some(11));
            assert_eq!(usage.completion_tokens, Some(7));
        }
        other => panic!("expected completed, got {other:?}"),
    }
    assert_eq!(result.attempts.len(), 1);
    let attempt = &result.attempts[0];
    assert_eq!(attempt.target_id, "deepseek");
    assert_eq!(attempt.requested_model, "deepseek-v4-flash");
    assert_eq!(attempt.returned_model.as_deref(), Some("deepseek-v4-flash"));
    assert_eq!(attempt.provider_request_id.as_deref(), Some("req-9"));
    assert_eq!(attempt.status, AttemptStatus::Completed);

    // 请求形状：json_object + stream:false + 无 api key + bearer。
    let req = &server.requests()[0];
    assert_eq!(req.path, "/v1/chat/completions");
    let body: serde_json::Value = serde_json::from_str(&req.body).unwrap();
    assert_eq!(body["response_format"]["type"], "json_object");
    assert_eq!(body["stream"], false);
    assert_eq!(body["max_tokens"], 8192);
    assert!(!req.body.contains("sk-deep"));
    assert!(req
        .headers
        .iter()
        .any(|(n, v)| n == "authorization" && v == "Bearer sk-deep"));
}

#[tokio::test]
async fn deepseek_flavor_adds_thinking_disabled_and_others_do_not() {
    let deepseek = MockServer::spawn().await;
    deepseek.enqueue(MockResponse::json(200, ok_body("{}")));
    let port = runtime(&deepseek, None, |p| p);
    port.complete(call(5_000)).await;
    let body: serde_json::Value = serde_json::from_str(&deepseek.requests()[0].body).unwrap();
    assert_eq!(body["thinking"]["type"], "disabled");

    let pixel = MockServer::spawn().await;
    pixel.enqueue(MockResponse::json(200, ok_body("{}")));
    // 用单 pixel 候选跑一次。
    let mut policies = policy::builtin_defaults("pixel");
    let mut base = policies
        .resolve(workload::ORGANIZER_STRUCTURED)
        .unwrap()
        .clone();
    base.candidates = vec!["pixel".to_owned()];
    policies.upsert(base).unwrap();
    let port = Arc::new(ModelRuntime::with_http_client(
        Arc::new(StaticSource::new(
            vec![pixel_target(pixel.endpoint())],
            vec![("pixel".to_owned(), "sk-pix".to_owned())],
        )),
        policies,
        no_proxy_client(),
    ));
    port.complete(call(5_000)).await;
    let body: serde_json::Value = serde_json::from_str(&pixel.requests()[0].body).unwrap();
    assert!(body.get("thinking").is_none());
}

#[tokio::test]
async fn rate_limited_retries_same_target_then_succeeds() {
    let server = MockServer::spawn().await;
    server.enqueue(MockResponse::json(429, "{\"error\":\"rate\"}").with_header("retry-after", "1"));
    server.enqueue(MockResponse::json(200, ok_body("{}")));
    let port = runtime(&server, None, |p| WorkloadPolicy {
        max_attempts: 2,
        ..p
    });
    let result = port.complete(call(5_000)).await;

    assert!(matches!(
        result.outcome,
        model_runtime::ModelOutcome::Completed { .. }
    ));
    assert_eq!(result.attempts.len(), 2);
    assert_eq!(
        result.attempts[0].status,
        AttemptStatus::Failed {
            code: ModelErrorCode::RateLimited
        }
    );
    assert_eq!(server.requests().len(), 2);
}

#[tokio::test]
async fn auth_error_does_not_retry_but_falls_back_when_allowed() {
    let primary = MockServer::spawn().await;
    primary.enqueue(MockResponse::json(401, "{\"error\":\"bad key\"}"));
    let secondary = MockServer::spawn().await;
    secondary.enqueue(MockResponse::json(200, ok_body("{}")));
    let port = runtime(&primary, Some(&secondary), |p| WorkloadPolicy {
        candidates: vec!["deepseek".to_owned(), "pixel".to_owned()],
        allow_fallback: true,
        max_attempts: 3,
        ..p
    });
    let result = port.complete(call(5_000)).await;

    assert!(matches!(
        result.outcome,
        model_runtime::ModelOutcome::Completed { .. }
    ));
    assert_eq!(result.attempts.len(), 2);
    assert_eq!(result.attempts[0].target_id, "deepseek");
    assert_eq!(result.attempts[1].target_id, "pixel");
    // 401 不对同 target 重试：主候选只收到一个请求。
    assert_eq!(primary.requests().len(), 1);
}

#[tokio::test]
async fn auth_error_fails_fast_without_fallback() {
    let primary = MockServer::spawn().await;
    primary.enqueue(MockResponse::json(401, "{\"error\":\"bad key\"}"));
    let port = runtime(&primary, None, |p| p);
    let result = port.complete(call(5_000)).await;

    assert_eq!(failed_code(&result), ModelErrorCode::Auth);
    assert_eq!(result.attempts.len(), 1);
    assert_eq!(primary.requests().len(), 1);
}

#[tokio::test]
async fn billing_error_is_user_actionable_and_keeps_the_provider_request_id() {
    let primary = MockServer::spawn().await;
    primary.enqueue(
        MockResponse::json(402, r#"{"error":{"message":"insufficient balance"}}"#)
            .with_header("x-request-id", "billing-request-42"),
    );
    let port = runtime(&primary, None, |p| p);
    let result = port.complete(call(5_000)).await;

    assert_eq!(failed_code(&result), ModelErrorCode::Billing);
    assert_eq!(result.attempts.len(), 1);
    assert_eq!(
        result.attempts[0].provider_request_id.as_deref(),
        Some("billing-request-42")
    );
    match result.outcome {
        model_runtime::ModelOutcome::Failed { error } => {
            assert_eq!(
                error.provider_request_id.as_deref(),
                Some("billing-request-42")
            );
        }
        other => panic!("unexpected outcome: {other:?}"),
    }
    assert_eq!(primary.requests().len(), 1);
}

#[tokio::test]
async fn server_error_retries_target_then_falls_back() {
    let primary = MockServer::spawn().await;
    primary.enqueue(MockResponse::json(500, "{\"error\":\"boom\"}"));
    primary.enqueue(MockResponse::json(500, "{\"error\":\"boom2\"}"));
    let secondary = MockServer::spawn().await;
    secondary.enqueue(MockResponse::json(200, ok_body("{}")));
    let port = runtime(&primary, Some(&secondary), |p| WorkloadPolicy {
        candidates: vec!["deepseek".to_owned(), "pixel".to_owned()],
        allow_fallback: true,
        max_attempts: 4,
        ..p
    });
    let result = port.complete(call(5_000)).await;

    assert!(matches!(
        result.outcome,
        model_runtime::ModelOutcome::Completed { .. }
    ));
    // 语义：retryable 错误先同 target 重试一次，再换候选。
    assert_eq!(result.attempts.len(), 3);
    let targets: Vec<&str> = result
        .attempts
        .iter()
        .map(|a| a.target_id.as_str())
        .collect();
    assert_eq!(targets, vec!["deepseek", "deepseek", "pixel"]);
}

#[tokio::test]
async fn truncated_finish_reason_fails_without_retry() {
    let server = MockServer::spawn().await;
    server.enqueue(MockResponse::json(
        200,
        serde_json::json!({
            "choices": [{"message": {"content": "{\"a\":"}, "finish_reason": "length"}]
        })
        .to_string(),
    ));
    let port = runtime(&server, None, |p| WorkloadPolicy {
        max_attempts: 3,
        ..p
    });
    let result = port.complete(call(5_000)).await;

    assert_eq!(failed_code(&result), ModelErrorCode::Truncated);
    assert_eq!(result.attempts.len(), 1);
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn content_filter_and_missing_finish_reason_fail() {
    let server = MockServer::spawn().await;
    server.enqueue(MockResponse::json(
        200,
        serde_json::json!({
            "choices": [{"message": {"content": "x"}, "finish_reason": "content_filter"}]
        })
        .to_string(),
    ));
    let port = runtime(&server, None, |p| p);
    assert_eq!(
        failed_code(&port.complete(call(5_000)).await),
        ModelErrorCode::ContentRejected
    );

    let server2 = MockServer::spawn().await;
    server2.enqueue(MockResponse::json(
        200,
        serde_json::json!({ "choices": [{"message": {"content": "x"}}] }).to_string(),
    ));
    let port2 = runtime(&server2, None, |p| p);
    assert_eq!(
        failed_code(&port2.complete(call(5_000)).await),
        ModelErrorCode::InvalidResponse
    );
}

#[tokio::test]
async fn invalid_envelope_reports_invalid_response() {
    let server = MockServer::spawn().await;
    server.enqueue(MockResponse::json(200, "<html>gateway home</html>"));
    let port = runtime(&server, None, |p| p);
    let result = port.complete(call(5_000)).await;
    assert_eq!(failed_code(&result), ModelErrorCode::InvalidResponse);
}

#[tokio::test]
async fn context_length_is_classified() {
    let server = MockServer::spawn().await;
    server.enqueue(MockResponse::json(
        400,
        "{\"error\":{\"message\":\"This model's maximum context length is 65536 tokens\"}}",
    ));
    let port = runtime(&server, None, |p| p);
    let result = port.complete(call(5_000)).await;
    assert_eq!(failed_code(&result), ModelErrorCode::ContextLength);
}

#[tokio::test]
async fn unsupported_response_format_is_classified() {
    let server = MockServer::spawn().await;
    server.enqueue(MockResponse::json(
        400,
        "{\"error\":{\"message\":\"response_format type json_object is not supported\"}}",
    ));
    let port = runtime(&server, None, |p| p);
    let result = port.complete(call(5_000)).await;
    assert_eq!(failed_code(&result), ModelErrorCode::Unsupported);
}

#[tokio::test]
async fn connection_reset_mid_response_is_network_error() {
    let server = MockServer::spawn().await;
    // hyper 对响应前断连会透明重试；队列耗尽后继续断连直到客户端放弃。
    server.enqueue(MockResponse::drop_connection());
    server.drop_when_empty();
    let port = runtime(&server, None, |p| p);
    let result = port.complete(call(5_000)).await;
    assert_eq!(failed_code(&result), ModelErrorCode::Network);
}

#[tokio::test]
async fn timeout_consumes_deadline_and_fails() {
    let server = MockServer::spawn().await;
    server.enqueue(MockResponse::hang());
    let port = runtime(&server, None, |p| p);
    let result = port.complete(call(300)).await;

    assert_eq!(failed_code(&result), ModelErrorCode::Timeout);
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn cancel_during_backoff_returns_cancelled() {
    let server = MockServer::spawn().await;
    server
        .enqueue(MockResponse::json(429, "{\"error\":\"rate\"}").with_header("retry-after", "60"));
    let port = runtime(&server, None, |p| WorkloadPolicy {
        max_attempts: 3,
        ..p
    });
    let flag = CancelFlag::new();
    let call = ModelCall {
        cancel: flag.clone(),
        deadline: Duration::from_secs(10),
        ..call(10_000)
    };

    let handle = tokio::spawn(async move { port.complete(call).await });
    // 等第一个请求到达后取消。
    for _ in 0..100 {
        if !server.requests().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    flag.cancel();
    let result = handle.await.unwrap();

    assert_eq!(failed_code(&result), ModelErrorCode::Cancelled);
    // 退避被打断，没有第二个请求。
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn deadline_exhausted_before_next_attempt() {
    let server = MockServer::spawn().await;
    server.enqueue(MockResponse::json(429, "{\"error\":\"rate\"}").with_header("retry-after", "2"));
    // max_attempts=3 但 retry-after=2s；call deadline 100ms → 退避期内耗尽。
    let port = runtime(&server, None, |p| WorkloadPolicy {
        max_attempts: 3,
        ..p
    });
    let result = port.complete(call(100)).await;

    assert_eq!(failed_code(&result), ModelErrorCode::RateLimited);
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn capability_skip_records_skipped_attempt() {
    let primary = MockServer::spawn().await;
    let secondary = MockServer::spawn().await;
    secondary.enqueue(MockResponse::json(200, ok_body("{}")));
    let mut primary_target = deepseek_target(primary.endpoint());
    // 主候选缺 StructuredOutput。
    primary_target.capabilities = BTreeSet::from([ProviderCapability::ChatCompletion]);
    let mut policies = policy::builtin_defaults("deepseek");
    let mut base = policies
        .resolve(workload::ORGANIZER_STRUCTURED)
        .unwrap()
        .clone();
    base.candidates = vec!["deepseek".to_owned(), "pixel".to_owned()];
    base.allow_fallback = true;
    policies.upsert(base).unwrap();
    let port = Arc::new(ModelRuntime::with_http_client(
        Arc::new(StaticSource::new(
            vec![primary_target, pixel_target(secondary.endpoint())],
            vec![
                ("deepseek".to_owned(), "sk-deep".to_owned()),
                ("pixel".to_owned(), "sk-pix".to_owned()),
            ],
        )),
        policies,
        no_proxy_client(),
    ));
    let result = port.complete(call(5_000)).await;

    assert!(matches!(
        result.outcome,
        model_runtime::ModelOutcome::Completed { .. }
    ));
    assert_eq!(result.attempts.len(), 2);
    assert!(
        matches!(&result.attempts[0].status, AttemptStatus::Skipped { reason } if reason.contains("lacks required"))
    );
    assert_eq!(result.attempts[1].target_id, "pixel");
    // 主候选未收到请求。
    assert!(primary.requests().is_empty());
}

#[tokio::test]
async fn explicit_deadline_overrides_builtin_policy_floor() {
    // 旧行为等价回归：organizer plan.timeout_seconds 可以大于内置 90s，
    // 显式 deadline 不被 policy 截断；未显式给定时才用 policy 兜底。
    let server = MockServer::spawn().await;
    server.enqueue(MockResponse::hang());
    let port = runtime(&server, None, |p| p);
    let result = port
        .complete(ModelCall {
            deadline: Duration::from_millis(200),
            ..call(120_000)
        })
        .await;
    // 显式 200ms 生效（而非 policy 90s）。
    assert_eq!(failed_code(&result), ModelErrorCode::Timeout);
}

#[tokio::test]
async fn unknown_workload_fails_unavailable() {
    let server = MockServer::spawn().await;
    let port = runtime(&server, None, |p| p);
    let call = ModelCall {
        workload: "nope.workload".to_owned(),
        ..call(1_000)
    };
    let result = port.complete(call).await;
    assert_eq!(failed_code(&result), ModelErrorCode::Unavailable);
    assert!(server.requests().is_empty());
}

#[tokio::test]
async fn no_targets_fails_unavailable() {
    let server = MockServer::spawn().await;
    let mut policies = policy::builtin_defaults("deepseek");
    let mut base = policies
        .resolve(workload::ORGANIZER_STRUCTURED)
        .unwrap()
        .clone();
    base.candidates = vec!["ghost".to_owned()];
    policies.upsert(base).unwrap();
    let port = Arc::new(ModelRuntime::with_http_client(
        Arc::new(StaticSource::new(vec![], vec![])),
        policies,
        no_proxy_client(),
    ));
    let result = port.complete(call(1_000)).await;
    assert_eq!(failed_code(&result), ModelErrorCode::Unavailable);
    assert_eq!(server.requests().len(), 0);
}

#[tokio::test]
async fn max_attempts_caps_total_across_targets() {
    let primary = MockServer::spawn().await;
    primary.enqueue(MockResponse::json(500, "{\"e\":1}"));
    primary.enqueue(MockResponse::json(500, "{\"e\":2}"));
    primary.enqueue(MockResponse::json(500, "{\"e\":3}"));
    let secondary = MockServer::spawn().await;
    secondary.enqueue(MockResponse::json(500, "{\"e\":1}"));
    let port = runtime(&primary, Some(&secondary), |p| WorkloadPolicy {
        candidates: vec!["deepseek".to_owned(), "pixel".to_owned()],
        allow_fallback: true,
        max_attempts: 3,
        ..p
    });
    let result = port.complete(call(5_000)).await;

    assert_eq!(failed_code(&result), ModelErrorCode::Server);
    let total = primary.requests().len() + secondary.requests().len();
    assert_eq!(
        total, 3,
        "attempts must be capped at max_attempts across targets"
    );
    assert_eq!(result.attempts.len(), 3);
}

#[tokio::test]
async fn fallback_disabled_sticks_to_primary_even_after_failure() {
    let primary = MockServer::spawn().await;
    primary.enqueue(MockResponse::json(500, "{\"e\":1}"));
    let secondary = MockServer::spawn().await;
    let port = runtime(&primary, Some(&secondary), |p| WorkloadPolicy {
        max_attempts: 3,
        ..p
    });
    let result = port.complete(call(5_000)).await;

    assert_eq!(failed_code(&result), ModelErrorCode::Server);
    assert!(secondary.requests().is_empty());
}

#[tokio::test]
async fn stream_forwards_sse_deltas_and_keeps_final_content() {
    let server = MockServer::spawn().await;
    server.enqueue(
        MockResponse::json(
            200,
            concat!(
                "data: {\"model\":\"deepseek-v4-flash\",\"choices\":[{\"delta\":{\"content\":\"{\\\"answer\\\":\\\"\"}}]}\n\n",
                "data: {\"choices\":[{\"delta\":{\"content\":\"hello\\\"}\"}}]}\n\n",
                "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
                "data: {\"usage\":{\"prompt_tokens\":4,\"completion_tokens\":2}}\n\n",
                "data: [DONE]\n\n"
            ),
        )
        .with_header("content-type", "text/event-stream"),
    );
    let port = runtime(&server, None, |p| p);
    let chunks = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let received = Arc::clone(&chunks);
    let sink: ModelTokenSink = Arc::new(move |chunk| {
        received.lock().expect("chunks").push(chunk);
    });
    let result = port.stream(call(5_000), sink).await;

    match result.outcome {
        model_runtime::ModelOutcome::Completed {
            content,
            finish_reason,
            usage,
            ..
        } => {
            assert_eq!(content, "{\"answer\":\"hello\"}");
            assert_eq!(finish_reason, "stop");
            assert_eq!(usage.and_then(|value| value.completion_tokens), Some(2));
        }
        other => panic!("expected streamed completion, got {other:?}"),
    }
    assert_eq!(
        chunks.lock().expect("chunks").as_slice(),
        &["{\"answer\":\"".to_owned(), "hello\"}".to_owned()]
    );
    let body: serde_json::Value = serde_json::from_str(&server.requests()[0].body).unwrap();
    assert_eq!(body["stream"], true);
}

#[tokio::test]
async fn stream_requires_done_marker_and_terminal_finish_reason() {
    let server = MockServer::spawn().await;
    server.enqueue(MockResponse::json(
        200,
        "data: {\"choices\":[{\"delta\":{\"content\":\"x\"}}]}\n\n",
    ));
    let port = runtime(&server, None, |p| p);
    let sink: ModelTokenSink = Arc::new(|_| {});
    let result = port.stream(call(5_000), sink).await;
    assert_eq!(failed_code(&result), ModelErrorCode::InvalidResponse);
}

#[tokio::test]
async fn stream_accepts_usage_only_terminal_event() {
    let server = MockServer::spawn().await;
    server.enqueue(
        MockResponse::json(
            200,
            concat!(
                "data: {\"choices\":[{\"delta\":{\"content\":\"{\\\"answer\\\":\\\"ok\\\"}\"}}]}\n\n",
                "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
                "data: {\"usage\":{\"prompt_tokens\":2,\"completion_tokens\":3},\"choices\":[]}\n\n",
                "data: [DONE]\n\n"
            ),
        )
        .with_header("content-type", "text/event-stream"),
    );
    let port = runtime(&server, None, |p| p);
    let result = port.stream(call(5_000), Arc::new(|_| {})).await;
    match result.outcome {
        model_runtime::ModelOutcome::Completed { content, usage, .. } => {
            assert_eq!(content, "{\"answer\":\"ok\"}");
            assert_eq!(usage.and_then(|value| value.completion_tokens), Some(3));
        }
        other => panic!("expected streamed completion, got {other:?}"),
    }
}
