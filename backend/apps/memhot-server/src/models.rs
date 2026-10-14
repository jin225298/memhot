use crate::{
    auth::User,
    db::Job,
    error::{Error, Result},
    hash, network, App,
};
use axum::{extract::State, http::HeaderMap as ApiHeaders, Json};
use model_runtime::{ChatTurn, ModelRequest, ModelTarget, OutputSpec};
use reqwest::header::{HeaderMap, HeaderValue};
use serde_json::{json, Value};

#[path = "models_agent.rs"]
pub(crate) mod agent;

#[path = "models_provider.rs"]
mod provider;
pub use provider::{asr_permit, config, load_hosted, permit, ModelPermits};

pub fn configured(c: &Value) -> bool {
    ["baseUrl", "model", "apiKey"]
        .iter()
        .all(|key| c[*key].as_str().is_some_and(|s| !s.trim().is_empty()))
}
pub fn header(headers: &mut HeaderMap, name: &'static str, value: &str) -> Result<()> {
    headers.insert(
        name,
        HeaderValue::from_str(value).map_err(|_| Error::bad("模型凭据格式无效"))?,
    );
    Ok(())
}
fn request(
    c: &Value,
    messages: &[ChatTurn],
    structured: bool,
) -> Result<(String, HeaderMap, Value)> {
    if !configured(c) {
        return Err(Error::new(
            axum::http::StatusCode::CONFLICT,
            "model_unconfigured",
            "请先在设置中配置模型",
        ));
    }
    let base = c["baseUrl"]
        .as_str()
        .unwrap_or_default()
        .trim_end_matches('/');
    let model = c["model"].as_str().unwrap_or_default();
    let key = c["apiKey"].as_str().unwrap_or_default();
    let mut headers = HeaderMap::new();
    let (url, body) = match c["protocol"].as_str().unwrap_or("openai") {
        "openai" => {
            header(&mut headers, "authorization", &format!("Bearer {key}"))?;
            let target = ModelTarget {
                id: "memhot".into(),
                provider: if model.starts_with("deepseek") {
                    "deepseek"
                } else {
                    "openai"
                }
                .into(),
                endpoint: base.into(),
                model: model.into(),
                revision: 1,
                capabilities: Default::default(),
            };
            let req = ModelRequest {
                messages: messages.to_vec(),
                output: OutputSpec::PlainText,
                max_tokens: Some(if structured { 8192 } else { 2048 }),
                temperature: Some(0.3),
            };
            (
                format!("{base}/chat/completions"),
                model_runtime::chat_completions::build_body_for_workload(
                    &target,
                    &req,
                    false,
                    model_runtime::policy::workload::GENERAL_CHAT,
                ),
            )
        }
        "anthropic" => {
            header(&mut headers, "x-api-key", key)?;
            header(&mut headers, "anthropic-version", "2023-06-01")?;
            let system = messages
                .iter()
                .filter(|m| m.role == "system")
                .map(|m| m.content.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            let turns: Vec<Value> = messages
                .iter()
                .filter(|m| m.role != "system")
                .map(|m| json!({"role":m.role,"content":m.content}))
                .collect();
            (
                format!("{base}/messages"),
                json!({"model":model,"system":system,"messages":turns,"max_tokens":if structured{8192}else{2048}}),
            )
        }
        "gemini" => {
            header(&mut headers, "x-goog-api-key", key)?;
            let system = messages
                .iter()
                .filter(|m| m.role == "system")
                .map(|m| m.content.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            let turns:Vec<Value>=messages.iter().filter(|m|m.role!="system").map(|m|json!({"role":if m.role=="assistant"{"model"}else{"user"},"parts":[{"text":m.content}]})).collect();
            let mut url = network::validate_url(base)?;
            url.path_segments_mut()
                .map_err(|_| Error::bad("模型地址无效"))?
                .push("models")
                .push(&format!("{model}:generateContent"));
            (
                url.to_string(),
                json!({"systemInstruction":{"parts":[{"text":system}]},"contents":turns,"generationConfig":{"maxOutputTokens":if structured{8192}else{2048},"temperature":0.3}}),
            )
        }
        _ => return Err(Error::bad("不支持此文本模型协议")),
    };
    Ok((url, headers, body))
}
pub fn decode(protocol: &str, value: &Value) -> Result<String> {
    let (content, finish) = match protocol {
        "openai" => (
            value["choices"][0]["message"]["content"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
            value["choices"][0]["finish_reason"]
                .as_str()
                .unwrap_or_default(),
        ),
        "anthropic" => (
            value["content"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|p| p["text"].as_str())
                        .collect::<Vec<_>>()
                        .join("")
                })
                .unwrap_or_default(),
            value["stop_reason"].as_str().unwrap_or_default(),
        ),
        "gemini" => (
            value["candidates"][0]["content"]["parts"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|p| p["text"].as_str())
                        .collect::<Vec<_>>()
                        .join("")
                })
                .unwrap_or_default(),
            value["candidates"][0]["finishReason"]
                .as_str()
                .unwrap_or_default(),
        ),
        _ => return Err(Error::bad("不支持此模型协议")),
    };
    if matches!(finish, "length" | "max_tokens" | "MAX_TOKENS") {
        return Err(Error::upstream(
            "truncated",
            "模型输出被截断，请调整模型或缩小研究范围",
        ));
    }
    if !matches!(finish, "stop" | "end_turn" | "stop_sequence" | "STOP")
        || content.trim().is_empty()
    {
        return Err(Error::upstream(
            "invalid_response",
            "模型没有返回完整有效的回答",
        ));
    }
    Ok(content)
}
pub async fn call(
    app: &App,
    user: &str,
    messages: Vec<ChatTurn>,
    structured: bool,
    job: Option<&Job>,
    purpose: &str,
) -> Result<String> {
    let request_id = job.map(|j| j.id.clone()).unwrap_or_else(crate::id);
    metered_call(app, user, messages, structured, job, &request_id, purpose).await
}
pub async fn call_idempotent(
    app: &App,
    user: &str,
    messages: Vec<ChatTurn>,
    structured: bool,
    request_id: &str,
    purpose: &str,
) -> Result<String> {
    if request_id.is_empty() || request_id.len() > 200 {
        return Err(Error::bad("缺少有效请求ID"));
    }
    metered_call(app, user, messages, structured, None, request_id, purpose).await
}
/// Only curated public edition inputs may reach this platform-owned call path.
pub async fn public_call(
    app: &App,
    edition_key: &str,
    purpose: &str,
    messages: Vec<ChatTurn>,
) -> Result<String> {
    if edition_key.is_empty() || edition_key.len() > 200 || purpose.is_empty() || purpose.len() > 80
    {
        return Err(Error::bad("公共编辑阶段标识无效"));
    }
    let c = app
        .hosted
        .get("text")
        .filter(|c| configured(c))
        .ok_or_else(|| {
            Error::new(
                axum::http::StatusCode::CONFLICT,
                "hosted_model_unconfigured",
                "公共编辑尚未配置平台文本模型",
            )
        })?;
    let (url, headers, body) = request(c, &messages, true)?;
    let key = hash(serde_json::to_string(&(edition_key, purpose))?.as_bytes());
    let fingerprint = hash(format!("{url}:{}:{body}", c["protocol"]).as_bytes());
    let _permit = app
        .concurrency
        .model(crate::concurrency::Lane::Digest, None)
        .await?;
    if let Some(content) =
        crate::public_usage::reserve(app, &key, &fingerprint, crate::public_usage::daily_limit()?)?
    {
        return Ok(content);
    }
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(90),
        network::json_post(&url, headers, body),
    )
    .await
    .map_err(|_| Error::upstream("timeout", "公共编辑模型调用超时"))
    .and_then(|result| result)
    .and_then(|value| decode(c["protocol"].as_str().unwrap_or("openai"), &value));
    crate::public_usage::settle(app, &key, &result)?;
    result
}
async fn metered_call(
    app: &App,
    user: &str,
    messages: Vec<ChatTurn>,
    structured: bool,
    job: Option<&Job>,
    request_id: &str,
    purpose: &str,
) -> Result<String> {
    let c = config(app, user, "text")?;
    let (url, headers, body) = request(&c, &messages, structured)?;
    let signature = hash(format!("{purpose}:{request_id}").as_bytes());
    let fingerprint = hash(format!("{url}:{}:{body}", c["protocol"]).as_bytes());
    let lane = job
        .map(|job| crate::concurrency::Lane::for_job(&job.kind))
        .unwrap_or(crate::concurrency::Lane::Chat);
    let _permit = permit(app, user, lane).await?;
    if let Some(content) =
        crate::ledger::reserve_fingerprinted(app, user, job, &signature, &fingerprint, "text", 1)?
    {
        return Ok(content);
    }
    let result = network::json_post(&url, headers, body)
        .await
        .and_then(|v| decode(c["protocol"].as_str().unwrap_or("openai"), &v));
    crate::ledger::settle_request(app, user, &signature, &result)?;
    result
}
pub fn parse_json(text: &str) -> Result<Value> {
    let t = text.trim();
    let t = t
        .strip_prefix("```json")
        .or_else(|| t.strip_prefix("```"))
        .unwrap_or(t)
        .trim();
    let t = t.strip_suffix("```").unwrap_or(t).trim();
    serde_json::from_str(t).map_err(|_| Error::upstream("invalid_response", "模型结构化输出无效"))
}
pub async fn test(
    State(app): State<App>,
    User(user): User,
    headers: ApiHeaders,
    Json(input): Json<Value>,
) -> Result<Json<Value>> {
    let request_id = crate::billing::api::request_key(&headers)?;
    let result = match input["kind"].as_str() {
        Some("text") => call_idempotent(
            &app,
            &user,
            vec![ChatTurn::user("Reply with OK only.")],
            false,
            &request_id,
            "test",
        )
        .await
        .map(|_| ()),
        Some("asr") => crate::audio::test_idempotent(&app, &user, &request_id).await,
        _ => return Err(Error::bad("模型测试类型无效")),
    };
    Ok(Json(match result {
        Ok(()) => json!({"ok":true,"message":"真实API请求已完成；连接可用"}),
        Err(e) => json!({"ok":false,"message":e.message}),
    }))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn real_protocol_envelopes_and_truncation() {
        assert_eq!(
            decode(
                "openai",
                &json!({"choices":[{"message":{"content":"a"},"finish_reason":"stop"}]})
            )
            .unwrap(),
            "a"
        );
        assert_eq!(
            decode(
                "anthropic",
                &json!({"content":[{"type":"text","text":"b"}],"stop_reason":"end_turn"})
            )
            .unwrap(),
            "b"
        );
        assert_eq!(
            decode(
                "gemini",
                &json!({"candidates":[{"content":{"parts":[{"text":"c"}]},"finishReason":"STOP"}]})
            )
            .unwrap(),
            "c"
        );
        assert_eq!(
            decode(
                "openai",
                &json!({"choices":[{"message":{"content":"partial"},"finish_reason":"length"}]})
            )
            .unwrap_err()
            .code,
            "truncated"
        );
        assert!(decode(
            "openai",
            &json!({"choices":[{"message":{"content":"missing stop"}}]})
        )
        .is_err());
    }
    #[test]
    fn secrets_are_headers_and_protocol_specific() {
        let c = json!({"protocol":"anthropic","baseUrl":"https://example.com/v1","model":"claude-test","apiKey":"secret"});
        let (url, headers, body) = request(
            &c,
            &[ChatTurn::system("rules"), ChatTurn::user("hi")],
            false,
        )
        .unwrap();
        assert!(url.ends_with("/messages"));
        assert_eq!(headers["x-api-key"], "secret");
        assert!(!body.to_string().contains("secret"));
        assert_eq!(body["system"], "rules");
    }
    #[test]
    fn structured_output_budget_is_8192_across_supported_protocols() {
        for (protocol, model) in [
            ("openai", "deepseek-flash"),
            ("openai", "gpt-test"),
            ("anthropic", "claude-test"),
            ("gemini", "gemini-test"),
        ] {
            let c = json!({"protocol":protocol,"baseUrl":"https://example.com/v1","model":model,"apiKey":"synthetic"});
            for (structured, expected) in [(true, 8192), (false, 2048)] {
                let (_, _, body) = request(&c, &[ChatTurn::user("test")], structured).unwrap();
                let actual = if protocol == "gemini" {
                    &body["generationConfig"]["maxOutputTokens"]
                } else {
                    &body["max_tokens"]
                };
                assert_eq!(actual.as_u64(), Some(expected), "{protocol}/{model}");
            }
        }
    }
}
