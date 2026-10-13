//! Chat Completions 协议 adapter：编解码、终态语义与错误分类。
//!
//! 供应商差异集中在本文件：DeepSeek 兼容基线与 MiMo 结构化工作负载参数。
//! 终态判定：只有 `finish_reason == "stop"` 且
//! content 非空才是成功；截断/拒绝/缺失终态都是失败。

use std::time::Duration;

use crate::catalog::ModelTarget;
use crate::contracts::{ModelError, ModelErrorCode, ModelRequest, OutputSpec, Usage};

#[derive(Debug, Clone)]
pub struct AdapterSuccess {
    pub content: String,
    pub finish_reason: String,
    pub usage: Option<Usage>,
    pub returned_model: Option<String>,
    pub provider_request_id: Option<String>,
}

/// 供应商 flavor：请求体的兼容扩展。新增 vendor 参数必须集中在此。
fn flavor_extra_body(target: &ModelTarget) -> Vec<(String, serde_json::Value)> {
    if target.provider == "deepseek" && target.model.to_ascii_lowercase().starts_with("deepseek") {
        // DeepSeek 默认开思考会显著增加结构化整理的失败率与成本；
        // 兼容基线沿用 organizer 既有行为（thinking disabled）。
        vec![(
            "thinking".to_owned(),
            serde_json::json!({ "type": "disabled" }),
        )]
    } else {
        Vec::new()
    }
}

/// Build an OpenAI-compatible chat-completions body. `stream` is kept explicit
/// so batch callers retain the existing `stream:false` contract while the SSE
/// path can opt in without duplicating vendor flavor handling.
pub fn build_body_with_stream(
    target: &ModelTarget,
    request: &ModelRequest,
    stream: bool,
) -> serde_json::Value {
    let messages: Vec<serde_json::Value> = request
        .messages
        .iter()
        .map(|turn| serde_json::json!({ "role": turn.role, "content": turn.content }))
        .collect();
    let mut body = serde_json::json!({
        "model": target.model,
        "messages": messages,
        "stream": stream,
    });
    if matches!(request.output, OutputSpec::Json) {
        body["response_format"] = serde_json::json!({ "type": "json_object" });
    }
    if let Some(max_tokens) = request.max_tokens {
        body["max_tokens"] = serde_json::Value::from(max_tokens);
    }
    if let Some(temperature) = request.temperature {
        body["temperature"] = serde_json::Value::from(temperature);
    }
    for (key, value) in flavor_extra_body(target) {
        body[key] = value;
    }
    body
}

pub fn build_body(target: &ModelTarget, request: &ModelRequest) -> serde_json::Value {
    build_body_with_stream(target, request, false)
}

/// Memory processing, transcript correction, report organization, and user
/// answers need predictable bounded output without visible deliberation.
/// MiMo's official endpoint defaults to deep thinking, whose tokens share the
/// completion budget. Apply the documented controls only to these workloads;
/// all other callers retain the existing provider behavior.
pub fn build_body_for_workload(
    target: &ModelTarget,
    request: &ModelRequest,
    stream: bool,
    workload: &str,
) -> serde_json::Value {
    let mut body = build_body_with_stream(target, request, stream);
    let official_mimo = target.model.to_ascii_lowercase().starts_with("mimo-")
        && reqwest::Url::parse(&target.endpoint)
            .ok()
            .is_some_and(|url| url.host_str() == Some("api.xiaomimimo.com"));
    let bounded_workload = workload.starts_with("memory02.")
        || matches!(
            workload,
            crate::policy::workload::TRANSCRIPT_CORRECT
                | crate::policy::workload::ORGANIZER_STRUCTURED
                | crate::policy::workload::PROJECT_ANSWER
                | crate::policy::workload::GENERAL_CHAT
                | crate::policy::workload::SCENE_JUDGE
        );
    if bounded_workload && official_mimo {
        body["thinking"] = serde_json::json!({"type":"disabled"});
        if let Some(max_tokens) = request.max_tokens {
            body.as_object_mut()
                .expect("request body is an object")
                .remove("max_tokens");
            body["max_completion_tokens"] = serde_json::Value::from(max_tokens);
        }
    }
    body
}

/// 4xx/5xx 状态分类。body 文本用于区分 400 的具体原因。
fn classify_status(status: reqwest::StatusCode, body_text: &str) -> ModelError {
    let code = status.as_u16();
    let billing_related = is_billing_related(body_text);
    match code {
        401 => ModelError::new(ModelErrorCode::Auth, "HTTP 401", false),
        402 => ModelError::new(
            ModelErrorCode::Billing,
            "HTTP 402: account balance or quota is unavailable",
            false,
        ),
        403 if billing_related => ModelError::new(
            ModelErrorCode::Billing,
            "HTTP 403: account balance or quota is unavailable",
            false,
        ),
        403 => ModelError::new(ModelErrorCode::Auth, "HTTP 403", false),
        408 | 504 => ModelError::new(ModelErrorCode::Timeout, format!("HTTP {code}"), true),
        429 if billing_related => ModelError::new(
            ModelErrorCode::Billing,
            "HTTP 429: account quota is exhausted",
            false,
        ),
        429 => ModelError::new(ModelErrorCode::RateLimited, format!("HTTP {code}"), true),
        500..=599 => ModelError::new(ModelErrorCode::Server, format!("HTTP {code}"), true),
        400 if billing_related => ModelError::new(
            ModelErrorCode::Billing,
            "HTTP 400: account balance or quota is unavailable",
            false,
        ),
        400 => {
            let lowered = body_text.to_ascii_lowercase();
            if lowered.contains("context length") || lowered.contains("maximum context") {
                ModelError::new(
                    ModelErrorCode::ContextLength,
                    "provider rejected request: context length exceeded",
                    false,
                )
            } else if lowered.contains("response_format") || lowered.contains("json_object") {
                ModelError::new(
                    ModelErrorCode::Unsupported,
                    "provider does not support the requested response format",
                    false,
                )
            } else {
                ModelError::new(
                    ModelErrorCode::InvalidResponse,
                    "provider rejected request",
                    false,
                )
            }
        }
        _ => ModelError::new(
            ModelErrorCode::InvalidResponse,
            format!("HTTP {code}"),
            false,
        ),
    }
}

fn is_billing_related(body_text: &str) -> bool {
    let lowered = body_text.to_lowercase();
    [
        "insufficient balance",
        "insufficient_balance",
        "insufficient quota",
        "insufficient_quota",
        "quota exhausted",
        "quota_exhausted",
        "billing",
        "account balance",
        "余额不足",
        "额度不足",
        "欠费",
    ]
    .iter()
    .any(|marker| lowered.contains(marker))
}

/// transport 错误分类（不透传可能含敏感 query 的原始串）。
fn classify_transport(error: &reqwest::Error) -> ModelError {
    if error.is_timeout() {
        ModelError::new(ModelErrorCode::Timeout, "request timed out", true)
    } else if error.is_connect() {
        ModelError::new(ModelErrorCode::Network, "connection failed", true)
    } else {
        // 断流/解码失败等：连接层问题按网络错误处理（可换候选）。
        ModelError::new(ModelErrorCode::Network, "transport error", true)
    }
}

fn retry_after_ms(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    let raw = headers.get("retry-after")?.to_str().ok()?;
    let seconds: f64 = raw.trim().parse().ok()?;
    Some((seconds * 1000.0).max(1.0) as u64)
}

fn provider_request_id(headers: &reqwest::header::HeaderMap) -> Option<String> {
    ["x-request-id", "x-ds-request-id"]
        .into_iter()
        .find_map(|name| headers.get(name))
        .and_then(|value| value.to_str().ok())
        .and_then(sanitize_request_id)
}

fn sanitize_request_id(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() || value.len() > 128 || !value.bytes().all(|byte| byte.is_ascii_graphic()) {
        return None;
    }
    Some(value.to_owned())
}

/// 终态语义：stop+非空 content 才成功。
fn parse_completion(envelope: &serde_json::Value) -> Result<AdapterSuccess, ModelError> {
    let choice = envelope
        .get("choices")
        .and_then(|choices| choices.get(0))
        .ok_or_else(|| ModelError::new(ModelErrorCode::InvalidResponse, "no choices", false))?;
    let finish_reason = choice
        .get("finish_reason")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_owned();
    match finish_reason.as_str() {
        "stop" => {}
        "length" => {
            return Err(ModelError::new(
                ModelErrorCode::Truncated,
                "finish_reason=length: output truncated",
                false,
            ))
        }
        "content_filter" => {
            return Err(ModelError::new(
                ModelErrorCode::ContentRejected,
                "finish_reason=content_filter",
                false,
            ))
        }
        other => {
            return Err(ModelError::new(
                ModelErrorCode::InvalidResponse,
                format!("unexpected finish_reason `{other}`"),
                false,
            ))
        }
    }
    let content = choice
        .get("message")
        .and_then(|message| message.get("content"))
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    if content.trim().is_empty() {
        return Err(ModelError::new(
            ModelErrorCode::InvalidResponse,
            "empty message content",
            false,
        ));
    }
    let usage = envelope.get("usage").and_then(|usage| {
        let prompt = usage.get("prompt_tokens").and_then(|v| v.as_u64());
        let completion = usage.get("completion_tokens").and_then(|v| v.as_u64());
        (prompt.is_some() || completion.is_some()).then_some(Usage {
            prompt_tokens: prompt,
            completion_tokens: completion,
        })
    });
    let returned_model = envelope
        .get("model")
        .and_then(|v| v.as_str())
        .map(ToOwned::to_owned);
    Ok(AdapterSuccess {
        content: content.to_owned(),
        finish_reason,
        usage,
        returned_model,
        provider_request_id: None,
    })
}

/// 执行一次 HTTP 调用。timeout 由 execution 层传入（剩余 deadline）。
pub async fn invoke(
    client: &reqwest::Client,
    target: &ModelTarget,
    credential: &str,
    request: &ModelRequest,
    timeout: Duration,
) -> Result<AdapterSuccess, ModelError> {
    invoke_for_workload(client, target, credential, request, timeout, "").await
}

pub async fn invoke_for_workload(
    client: &reqwest::Client,
    target: &ModelTarget,
    credential: &str,
    request: &ModelRequest,
    timeout: Duration,
    workload: &str,
) -> Result<AdapterSuccess, ModelError> {
    let url = format!("{}/chat/completions", target.endpoint);
    let body = build_body_for_workload(target, request, false, workload);
    let mut response = client
        .post(&url)
        .timeout(timeout)
        .bearer_auth(credential)
        .json(&body)
        .send()
        .await
        .map_err(|error| classify_transport(&error))?;

    let status = response.status();
    let headers = response.headers().clone();
    const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
    let too_large = || {
        ModelError::new(
            ModelErrorCode::InvalidResponse,
            "model response exceeds limit",
            false,
        )
    };
    if response
        .content_length()
        .is_some_and(|n| n > MAX_RESPONSE_BYTES as u64)
    {
        return Err(too_large());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| classify_transport(&error))?
    {
        if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
            return Err(too_large());
        }
        bytes.extend_from_slice(&chunk);
    }
    let text = String::from_utf8(bytes).map_err(|_| {
        ModelError::new(
            ModelErrorCode::InvalidResponse,
            "model response is not UTF-8",
            false,
        )
    })?;

    if !status.is_success() {
        let mut error = classify_status(status, &text);
        if let Some(request_id) = provider_request_id(&headers) {
            error = error.with_provider_request_id(request_id);
        }
        if error.code == ModelErrorCode::RateLimited {
            if let Some(ms) = retry_after_ms(&headers) {
                error = error.with_retry_after(ms);
            }
        }
        return Err(error);
    }

    let envelope: serde_json::Value = serde_json::from_str(&text).map_err(|_| {
        ModelError::new(
            ModelErrorCode::InvalidResponse,
            "provider returned invalid JSON",
            false,
        )
    })?;
    let mut success = parse_completion(&envelope)?;
    success.provider_request_id = provider_request_id(&headers);
    Ok(success)
}

/// Execute an OpenAI-compatible SSE chat-completions request.
///
/// MiMo exposes the same `data: {"choices":[{"delta":{"content":...}}]}`
/// envelope as OpenAI. We still validate the terminal `finish_reason` and
/// return the exact concatenated content, so structured project answers are
/// validated by the existing business layer after streaming. Network chunks
/// can split SSE lines; `pending` therefore buffers until a newline arrives.
pub async fn invoke_stream(
    client: &reqwest::Client,
    target: &ModelTarget,
    credential: &str,
    request: &ModelRequest,
    timeout: Duration,
    sink: crate::contracts::ModelTokenSink,
) -> Result<AdapterSuccess, ModelError> {
    invoke_stream_for_workload(client, target, credential, request, timeout, sink, "").await
}

pub async fn invoke_stream_for_workload(
    client: &reqwest::Client,
    target: &ModelTarget,
    credential: &str,
    request: &ModelRequest,
    timeout: Duration,
    sink: crate::contracts::ModelTokenSink,
    workload: &str,
) -> Result<AdapterSuccess, ModelError> {
    let url = format!("{}/chat/completions", target.endpoint);
    let body = build_body_for_workload(target, request, true, workload);
    let response = client
        .post(&url)
        .timeout(timeout)
        .bearer_auth(credential)
        .json(&body)
        .send()
        .await
        .map_err(|error| classify_transport(&error))?;

    let status = response.status();
    let headers = response.headers().clone();
    const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
    if !status.is_success() {
        let text = response
            .text()
            .await
            .map_err(|error| classify_transport(&error))?;
        let mut error = classify_status(status, &text);
        if let Some(request_id) = provider_request_id(&headers) {
            error = error.with_provider_request_id(request_id);
        }
        if error.code == ModelErrorCode::RateLimited {
            if let Some(ms) = retry_after_ms(&headers) {
                error = error.with_retry_after(ms);
            }
        }
        return Err(error);
    }

    let mut response = response;
    if response
        .content_length()
        .is_some_and(|n| n > MAX_RESPONSE_BYTES as u64)
    {
        return Err(ModelError::new(
            ModelErrorCode::InvalidResponse,
            "model response exceeds limit",
            false,
        ));
    }
    let mut pending = String::new();
    let mut assembled = String::new();
    let mut returned_model = None;
    let mut finish_reason = None;
    let mut usage = None;
    let mut received_done = false;
    let mut received_bytes = 0usize;

    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| classify_transport(&error))?
    {
        received_bytes = received_bytes.saturating_add(chunk.len());
        if received_bytes > MAX_RESPONSE_BYTES {
            return Err(ModelError::new(
                ModelErrorCode::InvalidResponse,
                "model response exceeds limit",
                false,
            ));
        }
        let text = std::str::from_utf8(&chunk).map_err(|_| {
            ModelError::new(
                ModelErrorCode::InvalidResponse,
                "model response is not UTF-8",
                false,
            )
        })?;
        pending.push_str(text);
        while let Some(newline) = pending.find('\n') {
            let line = pending[..newline].trim_end_matches('\r').to_owned();
            pending.drain(..=newline);
            if let Some(data) = line.strip_prefix("data:") {
                let data = data.trim();
                if data == "[DONE]" {
                    received_done = true;
                    continue;
                }
                if data.is_empty() {
                    continue;
                }
                let envelope: serde_json::Value = serde_json::from_str(data).map_err(|_| {
                    ModelError::new(
                        ModelErrorCode::InvalidResponse,
                        "provider returned invalid SSE JSON",
                        false,
                    )
                })?;
                if returned_model.is_none() {
                    returned_model = envelope
                        .get("model")
                        .and_then(|value| value.as_str())
                        .map(ToOwned::to_owned);
                }
                if usage.is_none() {
                    usage = envelope.get("usage").and_then(|value| {
                        let prompt = value.get("prompt_tokens").and_then(|v| v.as_u64());
                        let completion = value.get("completion_tokens").and_then(|v| v.as_u64());
                        (prompt.is_some() || completion.is_some()).then_some(Usage {
                            prompt_tokens: prompt,
                            completion_tokens: completion,
                        })
                    });
                }
                // OpenAI-compatible providers may append a usage-only SSE
                // event with `choices: []` before `[DONE]`. It carries no
                // delta or finish reason and must not invalidate the already
                // assembled answer.
                let Some(choices) = envelope
                    .get("choices")
                    .and_then(serde_json::Value::as_array)
                else {
                    // Some gateways omit `choices` on a usage-only frame.
                    if envelope.get("usage").is_some() {
                        continue;
                    }
                    return Err(ModelError::new(
                        ModelErrorCode::InvalidResponse,
                        "no choices",
                        false,
                    ));
                };
                let Some(choice) = choices.first() else {
                    continue;
                };
                if let Some(reason) = choice.get("finish_reason").and_then(|v| v.as_str()) {
                    finish_reason = Some(reason.to_owned());
                }
                if let Some(delta) = choice.get("delta") {
                    if let Some(content) = delta.get("content").and_then(|v| v.as_str()) {
                        if !content.is_empty() {
                            assembled.push_str(content);
                            sink(content.to_owned());
                        }
                    }
                }
            }
        }
    }
    // A compliant provider sends a final newline, but accepting a final
    // unterminated line makes the parser robust to proxies that flush EOF.
    if !pending.trim().is_empty() {
        let line = pending.trim_end_matches('\r');
        if let Some(data) = line.strip_prefix("data:") {
            let data = data.trim();
            if data == "[DONE]" {
                received_done = true;
            }
        }
    }

    let finish_reason = finish_reason.unwrap_or_default();
    if !received_done {
        return Err(ModelError::new(
            ModelErrorCode::InvalidResponse,
            "SSE stream ended before [DONE]",
            false,
        ));
    }
    match finish_reason.as_str() {
        "stop" => {}
        "length" => {
            return Err(ModelError::new(
                ModelErrorCode::Truncated,
                "finish_reason=length: output truncated",
                false,
            ))
        }
        "content_filter" => {
            return Err(ModelError::new(
                ModelErrorCode::ContentRejected,
                "finish_reason=content_filter",
                false,
            ))
        }
        other => {
            return Err(ModelError::new(
                ModelErrorCode::InvalidResponse,
                format!("unexpected finish_reason `{other}`"),
                false,
            ))
        }
    }
    if assembled.trim().is_empty() {
        return Err(ModelError::new(
            ModelErrorCode::InvalidResponse,
            "empty message content",
            false,
        ));
    }
    Ok(AdapterSuccess {
        content: assembled,
        finish_reason,
        usage,
        returned_model,
        provider_request_id: provider_request_id(&headers),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::ChatTurn;

    fn target(provider: &str) -> ModelTarget {
        ModelTarget {
            id: "t".to_owned(),
            provider: provider.to_owned(),
            endpoint: "https://x.example/v1".to_owned(),
            model: "m".to_owned(),
            revision: 1,
            capabilities: Default::default(),
        }
    }

    fn request() -> ModelRequest {
        ModelRequest {
            messages: vec![ChatTurn::system("s"), ChatTurn::user("u")],
            output: OutputSpec::Json,
            max_tokens: Some(8192),
            temperature: None,
        }
    }

    #[test]
    fn body_carries_json_mode_max_tokens_and_flavor() {
        let mut deepseek = target("deepseek");
        deepseek.model = "deepseek-chat".to_owned();
        let body = build_body(&deepseek, &request());
        assert_eq!(body["response_format"]["type"], "json_object");
        assert_eq!(body["max_tokens"], 8192);
        assert_eq!(body["stream"], false);
        assert_eq!(body["thinking"]["type"], "disabled");

        let mut mimo = target("deepseek");
        mimo.model = "mimo-v2.6-flash".to_owned();
        mimo.endpoint = "https://api.xiaomimimo.com/v1".to_owned();
        assert!(build_body(&mimo, &request()).get("thinking").is_none());

        let plain = build_body(
            &target("other"),
            &ModelRequest {
                output: OutputSpec::PlainText,
                temperature: Some(0.3),
                ..request()
            },
        );
        assert!(plain.get("response_format").is_none());
        assert!(plain.get("thinking").is_none());
        assert_eq!(plain["temperature"], 0.3);
    }

    #[test]
    fn official_mimo_bounded_controls_are_workload_scoped_and_host_checked() {
        let mut mimo = target("deepseek");
        mimo.model = "mimo-v2.6-flash".into();
        mimo.endpoint = "https://api.xiaomimimo.com/v1".into();
        for workload in [
            "memory02.formation",
            "memory02.recall",
            crate::policy::workload::TRANSCRIPT_CORRECT,
            crate::policy::workload::ORGANIZER_STRUCTURED,
            crate::policy::workload::PROJECT_ANSWER,
            crate::policy::workload::GENERAL_CHAT,
            crate::policy::workload::SCENE_JUDGE,
        ] {
            for stream in [false, true] {
                let body = build_body_for_workload(&mimo, &request(), stream, workload);
                assert_eq!(body["thinking"]["type"], "disabled");
                assert_eq!(body["max_completion_tokens"], 8192);
                assert!(body.get("max_tokens").is_none());
                assert_eq!(body["stream"], stream);
                assert_eq!(body["response_format"]["type"], "json_object");
            }
        }
        for workload in [
            "transcript.correct.extra",
            "organizer.structured.extra",
            "memory02_other",
            "",
        ] {
            assert_eq!(
                build_body_for_workload(&mimo, &request(), false, workload),
                build_body(&mimo, &request())
            );
        }
        for endpoint in [
            "https://api.xiaomimimo.com.evil.example/v1",
            "https://other.example/api.xiaomimimo.com/v1",
            "https://proxy.example/v1",
        ] {
            mimo.endpoint = endpoint.into();
            for workload in [
                "memory02.formation",
                crate::policy::workload::TRANSCRIPT_CORRECT,
                crate::policy::workload::ORGANIZER_STRUCTURED,
            ] {
                assert_eq!(
                    build_body_for_workload(&mimo, &request(), false, workload),
                    build_body(&mimo, &request())
                );
            }
        }
        mimo.endpoint = "https://api.xiaomimimo.com/v1".into();
        mimo.model = "another-model".into();
        for workload in [
            "memory02.formation",
            crate::policy::workload::TRANSCRIPT_CORRECT,
            crate::policy::workload::ORGANIZER_STRUCTURED,
        ] {
            assert_eq!(
                build_body_for_workload(&mimo, &request(), false, workload),
                build_body(&mimo, &request())
            );
        }
    }

    #[test]
    fn completion_requires_stop_and_content() {
        let ok = serde_json::json!({
            "choices": [{"message": {"content": "x"}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 1, "completion_tokens": 2}
        });
        assert!(parse_completion(&ok).is_ok());

        let no_finish = serde_json::json!({"choices": [{"message": {"content": "x"}}]});
        assert_eq!(
            parse_completion(&no_finish).unwrap_err().code,
            ModelErrorCode::InvalidResponse
        );

        let empty = serde_json::json!({"choices": [{"message": {"content": "  "}, "finish_reason": "stop"}]});
        assert_eq!(
            parse_completion(&empty).unwrap_err().code,
            ModelErrorCode::InvalidResponse
        );
    }

    #[test]
    fn billing_responses_are_distinct_from_auth_and_rate_limits() {
        assert_eq!(
            classify_status(reqwest::StatusCode::PAYMENT_REQUIRED, "{}").code,
            ModelErrorCode::Billing
        );
        assert_eq!(
            classify_status(
                reqwest::StatusCode::TOO_MANY_REQUESTS,
                r#"{"error":{"message":"insufficient quota"}}"#,
            )
            .code,
            ModelErrorCode::Billing
        );
        assert_eq!(
            classify_status(reqwest::StatusCode::TOO_MANY_REQUESTS, "slow down").code,
            ModelErrorCode::RateLimited
        );
        assert_eq!(
            classify_status(reqwest::StatusCode::UNAUTHORIZED, "bad key").code,
            ModelErrorCode::Auth
        );
    }

    #[test]
    fn provider_body_is_used_for_classification_but_never_exposed() {
        let error = classify_status(
            reqwest::StatusCode::BAD_REQUEST,
            "context length exceeded; authorization=secret-token",
        );
        assert_eq!(error.code, ModelErrorCode::ContextLength);
        assert!(!error.message.contains("secret-token"));
        assert!(!error.message.contains("authorization"));

        let invalid = ModelError::new(
            ModelErrorCode::InvalidResponse,
            "provider returned invalid JSON",
            false,
        );
        assert!(!invalid.message.contains("secret-token"));
    }

    #[test]
    fn request_ids_are_bounded_and_ascii_only() {
        assert_eq!(sanitize_request_id(" req-42 ").as_deref(), Some("req-42"));
        assert!(sanitize_request_id("contains whitespace").is_none());
        assert!(sanitize_request_id(&"x".repeat(129)).is_none());
    }
}
