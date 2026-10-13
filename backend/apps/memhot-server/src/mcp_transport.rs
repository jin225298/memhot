//! Bounded Pi MCP child. Rust authorizes every HTTP body against one trusted request.

use crate::{
    error::{Error, Result},
    network,
};
use reqwest::{header::HeaderMap, Method};
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    process::Stdio,
    sync::atomic::{AtomicI64, Ordering},
    time::Duration,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
const BODY: usize = 512 * 1024;
static AIHOT_RETRY_AT: AtomicI64 = AtomicI64::new(0);
static ALPHAXIV_RETRY_AT: AtomicI64 = AtomicI64::new(0);

#[derive(Clone, Debug)]
pub struct Request {
    pub(crate) tool: &'static str,
    pub(crate) args: Value,
}
impl Request {
    pub(crate) fn start(&self) -> Value {
        json!({"type":"start","tool":self.tool,"args":self.args})
    }
}

/// Trusted provider policy is never supplied by a model or MCP response.
pub(crate) struct Server<'a> {
    pub endpoint: &'static str,
    pub client: &'static str,
    pub script: &'static str,
    pub token: Option<&'a str>,
    pub seconds: u64,
    pub body_limit: usize,
}
impl Server<'_> {
    pub fn aihot() -> Self {
        Self {
            endpoint: crate::aihot::ENDPOINT,
            client: "memhot-aihot",
            script: "aihot-main.mjs",
            token: None,
            seconds: 30,
            body_limit: BODY,
        }
    }
}
pub(crate) fn validate_http(
    request: &Request,
    server: &Server<'_>,
    value: &Value,
) -> Result<(Method, HeaderMap, Option<String>)> {
    if value["url"] != server.endpoint {
        return Err(Error::bad("MCP 请求超出已配置的服务地址"));
    }
    let method = match value["method"].as_str() {
        Some("POST") => Method::POST,
        Some("DELETE") => Method::DELETE,
        _ => return Err(Error::bad("MCP 方法未授权")),
    };
    let fields = value["headers"]
        .as_object()
        .ok_or_else(|| Error::bad("MCP 请求头无效"))?;
    let mut headers = HeaderMap::new();
    for (name, field) in fields {
        if ![
            "accept",
            "content-type",
            "mcp-protocol-version",
            "mcp-session-id",
        ]
        .contains(&name.as_str())
        {
            return Err(Error::bad("MCP 请求头未授权"));
        }
        let text = field
            .as_str()
            .filter(|s| s.len() <= 512)
            .ok_or_else(|| Error::bad("MCP 请求头过大"))?;
        headers.insert(
            reqwest::header::HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| Error::bad("MCP 请求头无效"))?,
            reqwest::header::HeaderValue::from_str(text)
                .map_err(|_| Error::bad("MCP 请求头无效"))?,
        );
    }
    if method == Method::DELETE {
        if value.get("body").is_some() || !headers.contains_key("mcp-session-id") {
            return Err(Error::bad("MCP 会话关闭请求无效"));
        }
        return Ok((method, headers, None));
    }
    let raw = value["body"]
        .as_str()
        .filter(|v| v.len() <= 8192)
        .ok_or_else(|| Error::bad("MCP 请求正文无效"))?;
    let body: Value = serde_json::from_str(raw)?;
    if body["jsonrpc"] != "2.0" {
        return Err(Error::bad("MCP 协议无效"));
    }
    let allowed = match body["method"].as_str() {
        Some("initialize") => {
            body["params"]["capabilities"] == json!({})
                && body["params"]["clientInfo"] == json!({"name":server.client,"version":"0.3.3"})
                && matches!(
                    body["params"]["protocolVersion"].as_str(),
                    Some("2025-11-25" | "2025-06-18" | "2025-03-26" | "2024-11-05")
                )
        }
        Some("notifications/initialized") => {
            body["params"].is_null() || body["params"] == json!({})
        }
        Some("tools/list") => {
            body["params"].is_null()
                || body["params"] == json!({})
                || body["params"].as_object().is_some_and(|p| {
                    p.len() == 1
                        && p.get("cursor")
                            .and_then(Value::as_str)
                            .is_some_and(|s| s.len() <= 128)
                })
        }
        Some("tools/call") => {
            body["params"]["name"] == request.tool
                && body["params"]["arguments"] == request.args
                && body["params"].as_object().is_some_and(|p| p.len() == 2)
        }
        Some("notifications/cancelled") => {
            body["params"]["requestId"].is_number()
                && body["params"]["reason"]
                    .as_str()
                    .is_none_or(|s| s.len() <= 200)
        }
        _ => false,
    };
    if !allowed {
        return Err(Error::bad("MCP 请求超出本次工具与作用域"));
    }
    Ok((method, headers, Some(raw.into())))
}
async fn http(request: &Request, server: &Server<'_>, value: &Value) -> Result<Value> {
    let (method, mut headers, body) = validate_http(request, server, value)?;
    let retry_at = if server.token.is_some() {
        &ALPHAXIV_RETRY_AT
    } else {
        &AIHOT_RETRY_AT
    };
    if chrono::Utc::now().timestamp() < retry_at.load(Ordering::Relaxed) {
        return Err(Error::upstream("rate_limited", "资料服务要求稍后重试"));
    }
    if let Some(token) = server.token {
        let mut value = reqwest::header::HeaderValue::from_str(&format!("Bearer {token}"))
            .map_err(|_| Error::bad("论文服务凭据无效"))?;
        value.set_sensitive(true);
        headers.insert(reqwest::header::AUTHORIZATION, value);
    }
    let mut request = network::client_for(server.endpoint)
        .await?
        .request(method, server.endpoint)
        .headers(headers)
        .timeout(Duration::from_secs(if server.token.is_some() {
            45
        } else {
            15
        }));
    if let Some(body) = body {
        request = request.body(body);
    }
    let response = request.send().await.map_err(network::transport_error)?;
    if !response.status().is_success() {
        if matches!(response.status().as_u16(), 429 | 503) {
            let seconds = response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<i64>().ok())
                .unwrap_or(60)
                .clamp(1, 86400);
            retry_at.store(chrono::Utc::now().timestamp() + seconds, Ordering::Relaxed);
        }
        return Err(network::status_error(response.status()));
    }
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned();
    if content_type.len() > 200
        || (![202, 204].contains(&status)
            && !content_type.starts_with("application/json")
            && !content_type.starts_with("text/event-stream"))
    {
        return Err(Error::upstream("mcp_response", "MCP 返回类型无效"));
    }
    let mut headers = json!({});
    if let Some(id) = response
        .headers()
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
    {
        if id.len() > 512 {
            return Err(Error::bad("MCP 会话 ID 过大"));
        }
        headers["mcp-session-id"] = json!(id);
    }
    let body = String::from_utf8(network::bounded(response, server.body_limit).await?)
        .map_err(|_| Error::upstream("mcp_response", "MCP 返回无效文本"))?;
    Ok(
        json!({"status":status,"contentType":content_type,"headers":headers,"body":body,"url":server.endpoint}),
    )
}
async fn send(
    stdin: &mut tokio::process::ChildStdin,
    value: &Value,
    server: &Server<'_>,
    wire: &mut usize,
) -> Result<()> {
    let bytes = serde_json::to_vec(value)?;
    *wire += bytes.len();
    if bytes.len() > server.body_limit + 1024 * 1024 || *wire > server.body_limit * 3 + BODY {
        return Err(Error::upstream("response_too_large", "MCP 通信超限"));
    }
    stdin
        .write_all(&bytes)
        .await
        .map_err(|_| Error::internal())?;
    stdin
        .write_all(b"\n")
        .await
        .map_err(|_| Error::internal())?;
    stdin.flush().await.map_err(|_| Error::internal())
}
async fn execute(request: &Request, server: &Server<'_>) -> Result<Value> {
    static GATE: std::sync::OnceLock<tokio::sync::Semaphore> = std::sync::OnceLock::new();
    let _permit = GATE
        .get_or_init(|| tokio::sync::Semaphore::new(1))
        .acquire()
        .await
        .map_err(|_| Error::internal())?;
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let node = std::env::var_os("MEMHOT_AGENT_NODE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/opt/homebrew/bin/node"));
    let script = std::env::var_os("MEMHOT_AGENT_SCRIPT")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("services/memhot-agent/src/main.mjs"))
        .with_file_name(server.script);
    if [&node, &script]
        .iter()
        .any(|p| !p.is_absolute() || !p.is_file())
    {
        return Err(Error::upstream("mcp_unavailable", "MCP 运行时未配置"));
    }
    let mut child = tokio::process::Command::new(node)
        .arg("--max-old-space-size=96")
        .arg(script)
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| Error::upstream("mcp_unavailable", "MCP 无法启动"))?;
    let mut stdin = child.stdin.take().ok_or_else(Error::internal)?;
    let mut stdout = BufReader::new(child.stdout.take().ok_or_else(Error::internal)?);
    let mut wire = 0;
    let mut calls = std::collections::HashMap::<String, usize>::new();
    send(&mut stdin, &request.start(), server, &mut wire).await?;
    for _ in 0..9 {
        let mut bytes = Vec::new();
        loop {
            let buf = stdout.fill_buf().await.map_err(|_| Error::internal())?;
            if buf.is_empty() {
                return Err(Error::upstream("mcp_protocol", "MCP 提前退出"));
            }
            let count = buf
                .iter()
                .position(|b| *b == b'\n')
                .map_or(buf.len(), |i| i + 1);
            if bytes.len() + count > server.body_limit / 4 {
                return Err(Error::upstream("response_too_large", "MCP 输出超限"));
            }
            let done = buf[count - 1] == b'\n';
            bytes.extend_from_slice(&buf[..count]);
            stdout.consume(count);
            if done {
                break;
            }
        }
        wire += bytes.len();
        if wire > server.body_limit * 3 + BODY {
            return Err(Error::upstream("response_too_large", "MCP 累计输出超限"));
        }
        let value: Value = serde_json::from_slice(&bytes)?;
        match value["type"].as_str() {
            Some("http") => {
                let id = value["id"]
                    .as_str()
                    .filter(|s| !s.is_empty() && s.len() <= 100)
                    .ok_or_else(|| Error::bad("MCP 帧 ID 无效"))?;
                let body: Value = serde_json::from_str(value["body"].as_str().unwrap_or("{}"))?;
                let key = body["method"].as_str().unwrap_or("DELETE").to_owned();
                let count = calls.entry(key.clone()).or_default();
                *count += 1;
                if *count > if key == "tools/list" { 2 } else { 1 } {
                    return Err(Error::bad("MCP 操作次数超限"));
                }
                // SDK session cleanup is best effort and has a one-second cancellation
                // signal. Bound the host HTTP wait too, so it can read the completed result.
                validate_http(request, server, &value)?;
                let response = if value["method"] == "DELETE" {
                    match tokio::time::timeout(
                        Duration::from_millis(900),
                        http(request, server, &value),
                    )
                    .await
                    {
                        Ok(Ok(response)) => json!({"type":"result","id":id,"value":response}),
                        _ => json!({"type":"error","id":id,"code":"cleanup_failed"}),
                    }
                } else {
                    json!({"type":"result","id":id,"value":http(request, server, &value).await?})
                };
                if let Err(error) = send(&mut stdin, &response, server, &mut wire).await {
                    // The SDK may already have closed stdin after its one-second cleanup
                    // timeout. Still read its queued complete; bounds/protocol stay fatal.
                    if value["method"] != "DELETE" || error.code != "internal" {
                        return Err(error);
                    }
                }
            }
            Some("complete") => {
                drop(stdin);
                let status = tokio::time::timeout(Duration::from_secs(2), child.wait())
                    .await
                    .map_err(|_| Error::upstream("mcp_protocol", "MCP 完成后未退出"))?
                    .map_err(|_| Error::internal())?;
                if !status.success() {
                    return Err(Error::upstream("mcp_protocol", "MCP 异常退出"));
                }
                return Ok(value);
            }
            Some("failed")
                if matches!(
                    value["code"].as_str(),
                    Some(
                        "HTTP_FAILED"
                            | "HTTP_RESPONSE"
                            | "AIHOT_TOOL"
                            | "AIHOT_RESPONSE"
                            | "MCP_RESPONSE"
                            | "MCP_TOOL"
                            | "AGENT_FAILED"
                            | "RUN_TIMEOUT"
                            | "RPC_TIMEOUT"
                    )
                ) =>
            {
                return Err(Error::upstream("mcp_failed", "资料服务暂时无法取得资料"))
            }
            _ => {
                return Err(Error::upstream(
                    "mcp_protocol",
                    "MCP 返回未授权或无效协议帧",
                ))
            }
        }
    }
    Err(Error::upstream("mcp_protocol", "MCP 步数超限"))
}
pub(crate) async fn run(request: &Request, server: &Server<'_>) -> Result<Value> {
    tokio::time::timeout(
        Duration::from_secs(server.seconds),
        execute(request, server),
    )
    .await
    .map_err(|_| Error::upstream("timeout", "MCP 请求超时"))?
    .map_err(|error| {
        if error.code == "invalid_input" {
            Error::upstream("mcp_protocol", "MCP 返回未授权或无效协议帧")
        } else {
            error
        }
    })
}
