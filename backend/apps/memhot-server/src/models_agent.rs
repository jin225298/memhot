//! SDK provider envelopes remain intact; endpoint, credentials and budget stay in Rust.
use crate::{
    agent::Scope,
    error::{Error, Result},
    hash, network, App,
};
use reqwest::header::HeaderMap;
use serde_json::{json, Value};
#[path = "models_sse.rs"]
mod sse;
pub(crate) fn validate_config(c: &Value) -> Result<()> {
    if !super::configured(c) {
        return Err(Error::upstream("model_unconfigured", "文字服务尚未配置"));
    }
    if !matches!(
        c["protocol"].as_str().unwrap_or("openai"),
        "openai" | "anthropic" | "gemini"
    ) {
        return Err(Error::bad("不支持此文本模型协议"));
    }
    network::validate_url(c["baseUrl"].as_str().unwrap_or_default())?;
    Ok(())
}
fn request(c: &Value, mut body: Value, budget: usize) -> Result<(String, HeaderMap, Value)> {
    validate_config(c)?;
    if !body.is_object() || body.to_string().len() > 1024 * 1024 {
        return Err(Error::bad("代理模型请求无效"));
    }
    let protocol = c["protocol"].as_str().unwrap_or("openai");
    let allowed: &[&str] = match protocol {
        "openai" => &[
            "model",
            "messages",
            "tools",
            "tool_choice",
            "max_tokens",
            "max_completion_tokens",
            "temperature",
            "top_p",
            "stop",
            "seed",
            "response_format",
            "parallel_tool_calls",
            "frequency_penalty",
            "presence_penalty",
            "reasoning_effort",
            "verbosity",
            "stream",
            "stream_options",
            "thinking",
        ],
        "anthropic" => &[
            "model",
            "messages",
            "system",
            "tools",
            "tool_choice",
            "max_tokens",
            "temperature",
            "top_p",
            "top_k",
            "stop_sequences",
            "thinking",
            "stream",
        ],
        _ => &[
            "contents",
            "systemInstruction",
            "tools",
            "toolConfig",
            "generationConfig",
            "safetySettings",
        ],
    };
    let unknown: Vec<_> = body
        .as_object()
        .unwrap()
        .keys()
        .filter(|k| !allowed.contains(&k.as_str()))
        .map(|k| crate::clip(k, 60))
        .collect();
    if !unknown.is_empty() {
        return Err(Error::upstream(
            "agent_request_invalid",
            format!("SDK {protocol}请求包含未支持的字段：{}", unknown.join(",")),
        ));
    }
    let base = c["baseUrl"]
        .as_str()
        .unwrap_or_default()
        .trim_end_matches('/');
    let model = c["model"].as_str().unwrap_or_default();
    if protocol == "openai"
        && body.get("thinking").is_some()
        && (!model.to_ascii_lowercase().starts_with("deepseek")
            || body["thinking"] != json!({"type":"disabled"}))
    {
        return Err(Error::upstream(
            "agent_request_invalid",
            "SDK思考配置未通过平台限制",
        ));
    }
    let key = c["apiKey"].as_str().unwrap_or_default();
    let mut headers = HeaderMap::new();
    let url = match protocol {
        "openai" => {
            super::header(&mut headers, "authorization", &format!("Bearer {key}"))?;
            body["model"] = json!(model);
            body["stream"] = json!(true);
            if model.to_ascii_lowercase().starts_with("deepseek") {
                body["max_tokens"] = json!(budget);
                body.as_object_mut()
                    .unwrap()
                    .remove("max_completion_tokens");
            } else if body.get("max_completion_tokens").is_some() {
                body["max_completion_tokens"] = json!(budget);
                body.as_object_mut().unwrap().remove("max_tokens");
            } else {
                body["max_tokens"] = json!(budget);
            }
            format!("{base}/chat/completions")
        }
        "anthropic" => {
            super::header(&mut headers, "x-api-key", key)?;
            super::header(&mut headers, "anthropic-version", "2023-06-01")?;
            body["model"] = json!(model);
            body["max_tokens"] = json!(budget);
            body["stream"] = json!(true);
            format!("{base}/messages")
        }
        _ => {
            super::header(&mut headers, "x-goog-api-key", key)?;
            if !body["generationConfig"].is_object() {
                body["generationConfig"] = json!({});
            }
            body["generationConfig"]["maxOutputTokens"] = json!(budget);
            let mut url = network::validate_url(base)?;
            url.path_segments_mut()
                .map_err(|_| Error::bad("模型地址无效"))?
                .push("models")
                .push(&format!("{model}:streamGenerateContent"));
            url.query_pairs_mut().append_pair("alt", "sse");
            url.to_string()
        }
    };
    Ok((url, headers, body))
}
#[cfg(test)]
fn valid_response(protocol: &str, v: &Value) -> Result<()> {
    let finish = match protocol {
        "openai" => v["choices"][0]["finish_reason"].as_str(),
        "anthropic" => v["stop_reason"].as_str(),
        _ => v["candidates"][0]["finishReason"].as_str(),
    }
    .unwrap_or("");
    if matches!(finish, "length" | "max_tokens" | "MAX_TOKENS") {
        return Err(Error::upstream(
            "truncated",
            "模型输出达到上限，未发布不完整回答",
        ));
    }
    let valid = match protocol {
        "openai" => {
            matches!(finish, "stop" | "tool_calls" | "function_call")
                && (v["choices"][0]["message"]["content"]
                    .as_str()
                    .is_some_and(|s| !s.trim().is_empty())
                    || v["choices"][0]["message"]["tool_calls"]
                        .as_array()
                        .is_some_and(|a| !a.is_empty()))
        }
        "anthropic" => {
            matches!(finish, "end_turn" | "stop_sequence" | "tool_use")
                && v["content"].as_array().is_some_and(|a| !a.is_empty())
        }
        _ => {
            finish == "STOP"
                && v["candidates"][0]["content"]["parts"]
                    .as_array()
                    .is_some_and(|a| !a.is_empty())
        }
    };
    if !valid {
        return Err(Error::upstream(
            "invalid_response",
            "模型没有返回有效文本或工具调用",
        ));
    }
    Ok(())
}
pub(crate) async fn raw_call(
    app: &App,
    scope: &Scope,
    run_key: &str,
    index: usize,
    c: &Value,
    body: Value,
    budget: usize,
) -> Result<Value> {
    scope.check(app)?;
    let (url, headers, body) = request(c, body, budget)?;
    let key = hash(format!("{run_key}:model:{index}").as_bytes());
    let fingerprint = hash(format!("{url}:{}:{body}", c["protocol"]).as_bytes());
    let protocol = c["protocol"].as_str().unwrap_or("openai");
    let perform = async {
        let response = tokio::time::timeout(network::MODEL_TIMEOUT, async {
            let response = network::model_client_for(&url)
                .await?
                .post(&url)
                .headers(headers)
                .json(&body)
                .send()
                .await
                .map_err(network::transport_error)?;
            if !response.status().is_success() {
                return Err(network::status_error(response.status()));
            }
            let content_type = response
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase();
            if content_type != "text/event-stream" {
                return Err(Error::upstream("invalid_response", "模型未返回事件流"));
            }
            // SSE carries provider metadata per token; an 8192-token final report
            // can exceed 512 KiB even though its actual answer is only tens of KiB.
            let bytes = network::bounded(response, 4 * 1024 * 1024).await?;
            let body = String::from_utf8(bytes)
                .map_err(|_| Error::upstream("invalid_response", "模型流不是有效UTF-8"))?;
            sse::validate(protocol, &body).inspect_err(|_| {
                // Only protocol shape is observable: never retain rejected response
                // text or provider error messages, which may contain private prompts.
                let diagnostic = sse::diagnostic(protocol, &body);
                eprintln!(
                    "model_stream_rejected shape={} bytes={} json_events={} error_event={}",
                    diagnostic.shape,
                    diagnostic.bytes,
                    diagnostic.json_events,
                    diagnostic.error_event
                );
            })?;
            Ok(json!({"body":body,"contentType":content_type}))
        })
        .await
        .map_err(|_| Error::upstream("timeout", "代理模型调用超时"))??;
        Ok(serde_json::to_string(&response)?)
    };
    let raw = match scope {
        Scope::Private { user, job, .. } => {
            let _permit = super::permit(app, user, scope.lane()).await?;
            scope.check(app)?;
            if let Some(raw) = crate::ledger::reserve_fingerprinted(
                app,
                user,
                job.as_ref(),
                &key,
                &fingerprint,
                "text",
                1,
            )? {
                return serde_json::from_str(&raw).map_err(Into::into);
            }
            let result: Result<String> = match perform.await {
                result if scope.check(app).is_ok() => result,
                _ => Err(Error::missing()),
            };
            crate::ledger::settle_request(app, user, &key, &result)?;
            if let Err(error) = scope.check(app) {
                app.store.conn()?.execute(
                    "UPDATE receipts SET response=NULL WHERE user_id=? AND key=?",
                    rusqlite::params![user, key],
                )?;
                return Err(error);
            }
            result?
        }
        Scope::Public { .. } => {
            let _permit = app.concurrency.model(scope.lane(), None).await?;
            if let Some(raw) = crate::public_usage::reserve(
                app,
                &key,
                &fingerprint,
                crate::public_usage::daily_limit()?,
            )? {
                return serde_json::from_str(&raw).map_err(Into::into);
            }
            let result: Result<String> = perform.await;
            if let Err(error) = &result {
                eprintln!("public_model={} code={}", key, error.code);
            }
            crate::public_usage::settle(app, &key, &result)?;
            result?
        }
    };
    scope.check(app)?;
    serde_json::from_str(&raw).map_err(Into::into)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn deepseek_native_disabled_thinking_and_max_tokens_are_supported() {
        let c = json!({"protocol":"openai","baseUrl":"https://api.deepseek.com","model":"deepseek-v4-flash","apiKey":"secret"});
        let (_,_,body)=request(&c,json!({"model":"deepseek-v4-flash","messages":[],"stream":true,"stream_options":{"include_usage":true},"max_tokens":8192,"tools":[],"thinking":{"type":"disabled"}}),4096).unwrap();
        assert_eq!(body["thinking"], json!({"type":"disabled"}));
        assert_eq!(body["max_tokens"], 4096);
        assert!(request(
            &c,
            json!({"messages":[],"thinking":{"type":"enabled","budget_tokens":999999}}),
            4096
        )
        .is_err());
    }
    #[test]
    fn raw_sse_requires_a_complete_provider_finish_and_rejects_paid_truncation() {
        for (protocol,complete) in [
            ("openai","data: {\"choices\":[{\"delta\":{\"content\":\"ok\"},\"finish_reason\":null}]}\n\ndata: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n"),
            ("anthropic","event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"}}\n\n"),
            ("gemini","data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"ok\"}]},\"finishReason\":\"STOP\"}]}\n\n"),
        ] {assert!(sse::validate(protocol,complete).is_ok(),"{protocol}");}
        assert_eq!(
            sse::validate(
                "openai",
                "data: {\"choices\":[{\"finish_reason\":\"length\"}]}\n\n"
            )
            .unwrap_err()
            .code,
            "truncated"
        );
        assert_eq!(
            sse::validate("openai", "data: [DONE]\n\n")
                .unwrap_err()
                .code,
            "invalid_response"
        );
        assert_eq!(
            sse::validate(
                "anthropic",
                "data: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\"}}\n\n"
            )
            .unwrap_err()
            .code,
            "invalid_response"
        );
    }
    #[test]
    fn authority_is_fixed_and_tool_envelopes_are_retained() {
        let c = json!({"protocol":"openai","baseUrl":"https://example.com/v1","model":"trusted","apiKey":"secret"});
        let (url,h,b)=request(&c,json!({"model":"other","messages":[],"max_tokens":99999,"tools":[{"type":"function","function":{"name":"search"}}]}),2048).unwrap();
        assert_eq!(url, "https://example.com/v1/chat/completions");
        assert_eq!(h["authorization"], "Bearer secret");
        assert_eq!(b["model"], "trusted");
        assert_eq!(b["max_tokens"], 2048);
        assert!(!b.to_string().contains("secret"));
        assert!(request(&c, json!({"endpoint":"http://localhost","messages":[]}), 20).is_err());
        assert!(valid_response("openai",&json!({"choices":[{"finish_reason":"tool_calls","message":{"tool_calls":[{"id":"a","function":{"name":"search","arguments":"{}"}}]}}]})).is_ok());
        assert_eq!(
            valid_response(
                "gemini",
                &json!({"candidates":[{"finishReason":"MAX_TOKENS"}]})
            )
            .unwrap_err()
            .code,
            "truncated"
        );
    }
}
