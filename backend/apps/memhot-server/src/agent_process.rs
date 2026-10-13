use super::{state, tools, Scope};
use crate::{
    error::{Error, Result},
    App,
};
use serde_json::{json, Value};
use std::{path::PathBuf, process::Stdio};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
const LINE_LIMIT: usize = 1024 * 1024;
// The host may return a 4 MiB raw SSE stream, with JSON escaping overhead.
const HOST_FRAME_LIMIT: usize = 8 * 1024 * 1024;
// Full native context is emitted at each bounded turn, so the run budget exceeds a single frame.
const WIRE_LIMIT: usize = 16 * 1024 * 1024;
fn paths() -> Result<(PathBuf, PathBuf)> {
    let default_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let node = std::env::var_os("MEMHOT_AGENT_NODE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/opt/homebrew/bin/node"));
    let script = std::env::var_os("MEMHOT_AGENT_SCRIPT")
        .map(PathBuf::from)
        .unwrap_or_else(|| default_root.join("services/memhot-agent/src/main.mjs"));
    for path in [&node, &script] {
        if !path.is_absolute() || !std::fs::metadata(path).is_ok_and(|m| m.is_file()) {
            return Err(Error::upstream(
                "agent_unavailable",
                "研究服务尚未就绪，请稍后重试或联系管理员",
            ));
        }
    }
    Ok((node, script))
}
async fn send(stdin: &mut tokio::process::ChildStdin, frame: &Value) -> Result<()> {
    let mut bytes = serde_json::to_vec(frame)?;
    if bytes.len() > HOST_FRAME_LIMIT {
        return Err(Error::upstream(
            "response_too_large",
            "代理消息超出大小限制",
        ));
    }
    bytes.push(b'\n');
    stdin
        .write_all(&bytes)
        .await
        .map_err(|_| Error::upstream("agent_protocol", "代理输入通道已关闭"))?;
    stdin.flush().await.map_err(|_| Error::internal())
}
async fn frame<R: tokio::io::AsyncBufRead + Unpin>(reader: &mut R) -> Result<Value> {
    let mut bytes = Vec::new();
    loop {
        let buf = reader
            .fill_buf()
            .await
            .map_err(|_| Error::upstream("agent_protocol", "无法读取代理输出"))?;
        if buf.is_empty() {
            return Err(Error::upstream("agent_protocol", "代理在完成前退出"));
        }
        let n = buf
            .iter()
            .position(|b| *b == b'\n')
            .map(|i| i + 1)
            .unwrap_or(buf.len());
        if bytes.len() + n > LINE_LIMIT {
            return Err(Error::upstream(
                "response_too_large",
                "代理输出行超出大小限制",
            ));
        }
        let done = buf[n - 1] == b'\n';
        bytes.extend_from_slice(&buf[..n]);
        reader.consume(n);
        if done {
            break;
        }
    }
    serde_json::from_slice(&bytes)
        .map_err(|_| Error::upstream("agent_protocol", "代理输出格式无效"))
}
fn replay(old: &Value, request: &Value) -> Result<Value> {
    if old["request"] != *request {
        return Err(Error::upstream(
            "idempotency_conflict",
            "代理恢复请求与检查点不一致，已阻止重复调用",
        ));
    }
    Ok(old["response"].clone())
}
pub(super) async fn execute(
    app: &App,
    scope: &Scope,
    purpose: &str,
    config: &Value,
    saved: &mut state::Saved,
) -> Result<String> {
    let (node, script) = paths()?;
    let max = saved.input["maxSteps"].as_u64().unwrap_or(1) as usize;
    if saved.step >= max {
        return Err(Error::upstream("agent_step_limit", "代理已达到步骤上限"));
    }
    let mut child = tokio::process::Command::new(node)
        .arg("--max-old-space-size=128")
        .arg(script)
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| Error::upstream("agent_unavailable", "研究服务暂时无法启动，请稍后重试"))?;
    let mut stdin = child.stdin.take().ok_or_else(Error::internal)?;
    let mut stdout = BufReader::new(child.stdout.take().ok_or_else(Error::internal)?);
    let base = saved.step;
    let history = saved.messages_json.clone();
    let mut start = saved.input.clone();
    start["maxSteps"] = json!(max - base);
    if !history.is_empty() {
        start["messagesJson"] = json!(history);
        start["prompt"] = json!("");
    }
    send(&mut stdin, &start).await?;
    let (mut model_count, mut model_slot, mut tool_slot, mut tool_count) =
        (base, 0usize, 0usize, 0usize);
    let mut wire_bytes = 0usize;
    for _ in 0..128 {
        scope.check(app)?;
        let value = frame(&mut stdout).await?;
        wire_bytes += value.to_string().len();
        if wire_bytes > WIRE_LIMIT {
            return Err(Error::upstream(
                "response_too_large",
                "代理累计输出超出大小限制",
            ));
        }
        match value["type"].as_str().unwrap_or("") {
            "model" => {
                if model_count >= max {
                    return Err(Error::upstream(
                        "agent_step_limit",
                        "代理已达到模型调用上限",
                    ));
                }
                let id = value["id"]
                    .as_str()
                    .filter(|s| s.len() <= 100)
                    .ok_or_else(|| Error::bad("代理帧ID无效"))?;
                let request = value["body"].clone();
                let result = if let Some(old) = saved.pending_models.get(model_slot) {
                    replay(old, &request)?
                } else {
                    let raw = crate::models::agent::raw_call(
                        app,
                        scope,
                        &scope.key(purpose),
                        model_count,
                        config,
                        request.clone(),
                        saved.input["maxOutputTokens"].as_u64().unwrap_or(8192) as usize,
                    )
                    .await?;
                    saved
                        .pending_models
                        .push(json!({"request":request,"response":raw}));
                    state::persist(app, scope, purpose, saved)?;
                    raw
                };
                send(&mut stdin, &json!({"type":"result","id":id,"value":result})).await?;
                model_count += 1;
                model_slot += 1;
            }
            "tool" => {
                tool_count += 1;
                if tool_count > 32 || saved.input["tools"].as_array().is_none_or(Vec::is_empty) {
                    return Err(Error::bad("代理工具调用超出权限或上限"));
                }
                let id = value["id"]
                    .as_str()
                    .filter(|s| s.len() <= 100)
                    .ok_or_else(|| Error::bad("代理帧ID无效"))?;
                let name = value["name"].as_str().unwrap_or("");
                let args = &value["args"];
                let request = json!({"name":name,"args":args});
                let response = if let Some(old) = saved.pending_tools.get(tool_slot) {
                    replay(old, &request)?
                } else {
                    let response =
                        match tools::execute(app, scope, purpose, saved, name, args).await {
                            Ok(v) => json!({"type":"result","value":v}),
                            Err(e)
                                if matches!(
                                    e.code,
                                    "not_found"
                                        | "invalid_input"
                                        | "network"
                                        | "timeout"
                                        | "upstream"
                                        | "provider_auth"
                                        | "provider_rejected"
                                        | "rate_limited"
                                        | "web_plugin_failed"
                                        | "web_text_unavailable"
                                ) =>
                            {
                                json!({"type":"error","code":e.code,"message":e.message})
                            }
                            Err(e) => return Err(e),
                        };
                    saved
                        .pending_tools
                        .push(json!({"request":request,"response":response}));
                    state::persist(app, scope, purpose, saved)?;
                    response
                };
                let mut response = response;
                response["id"] = json!(id);
                send(&mut stdin, &response).await?;
                tool_slot += 1;
            }
            "checkpoint" => {
                let step = value["step"]
                    .as_u64()
                    .ok_or_else(|| Error::bad("检查点步骤无效"))?
                    as usize;
                if base + step != model_count || base + step <= saved.step || model_count > max {
                    return Err(Error::upstream("agent_protocol", "代理检查点步骤不一致"));
                }
                let messages = value["messagesJson"]
                    .as_str()
                    .filter(|value| value.len() <= 512 * 1024)
                    .ok_or_else(|| Error::bad("检查点消息无效"))?;
                // Pi system sections have meaningful insertion order. Preserve its native
                // transcript verbatim; parsing into serde_json::Value would reorder keys.
                let valid =
                    serde_json::from_str::<Value>(messages).is_ok_and(|value| value.is_array());
                if !valid {
                    return Err(Error::bad("检查点消息格式无效"));
                }
                saved.messages_json = messages.to_owned();
                saved.step = base + step;
                saved.pending_models.clear();
                saved.pending_tools.clear();
                model_slot = 0;
                tool_slot = 0;
                if value["finished"] == true {
                    if let Some(text) = value["text"].as_str().filter(|s| !s.trim().is_empty()) {
                        saved.complete = Some(text.to_owned());
                    }
                }
                state::persist(app, scope, purpose, saved)?;
            }
            "complete" => {
                let text = value["text"]
                    .as_str()
                    .filter(|s| !s.trim().is_empty())
                    .ok_or_else(|| Error::upstream("invalid_response", "代理没有返回有效回答"))?
                    .to_owned();
                if text.len() > 128 * 1024 {
                    return Err(Error::upstream(
                        "response_too_large",
                        "代理最终回答超出大小限制",
                    ));
                }
                drop(stdin);
                let status = tokio::time::timeout(std::time::Duration::from_secs(5), child.wait())
                    .await
                    .map_err(|_| Error::upstream("agent_protocol", "代理完成后未正常退出"))?
                    .map_err(|_| Error::internal())?;
                if !status.success() {
                    return Err(Error::upstream("agent_protocol", "代理进程异常退出"));
                }
                scope.check(app)?;
                saved.step = model_count;
                saved.complete = Some(text.clone());
                state::persist(app, scope, purpose, saved)?;
                return Ok(text);
            }
            "failed" => {
                return Err(Error::upstream(
                    match value["code"].as_str().unwrap_or("") {
                        "truncated" | "OUTPUT_TRUNCATED" => "truncated",
                        "step_limit" | "STEP_LIMIT" => "agent_step_limit",
                        "OUTPUT_LIMIT" | "CONTEXT_LIMIT" => "response_too_large",
                        "RPC_TIMEOUT" | "RUN_TIMEOUT" => "timeout",
                        _ => "agent_failed",
                    },
                    "这次内容没有整理完成，请稍后重试",
                ))
            }
            _ => return Err(Error::upstream("agent_protocol", "代理返回了未知帧")),
        }
    }
    Err(Error::upstream("agent_protocol", "代理帧数超出上限"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn jsonl_rejects_malformed_and_oversized_frames_before_allocation_grows() {
        let (mut writer, reader) = tokio::io::duplex(64);
        tokio::spawn(async move {
            let _ = writer.write_all(b"{invalid}\n").await;
        });
        assert_eq!(
            frame(&mut BufReader::new(reader)).await.unwrap_err().code,
            "agent_protocol"
        );
        let (mut writer, reader) = tokio::io::duplex(4096);
        tokio::spawn(async move {
            let _ = writer.write_all(&vec![b'x'; LINE_LIMIT + 1]).await;
        });
        assert_eq!(
            frame(&mut BufReader::new(reader)).await.unwrap_err().code,
            "response_too_large"
        );
    }
    #[test]
    fn pending_replay_requires_exact_request_and_preserves_raw_response() {
        let raw = json!({"body":"data: raw\n\n","contentType":"text/event-stream"});
        let pending =
            json!({"request":{"messages":[{"role":"user","content":"frozen"}]},"response":raw});
        assert_eq!(replay(&pending, &pending["request"]).unwrap(), raw);
        assert_eq!(
            replay(&pending, &json!({"messages":[]})).unwrap_err().code,
            "idempotency_conflict"
        );
    }
}
