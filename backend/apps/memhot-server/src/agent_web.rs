//! Pi web plugin RPC: Node extracts text; this host owns every network request.
use crate::{
    discovery::Material,
    error::{Error, Result},
    network,
};
use reqwest::{header::HeaderMap, Method, Url};
use serde_json::{json, Value};
use std::{path::PathBuf, process::Stdio, time::Duration};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader};

const INPUT_LIMIT: usize = 12 * 1024 * 1024;
const OUTPUT_LIMIT: usize = 256 * 1024;
const WIRE_LIMIT: usize = 16 * 1024 * 1024;
const BODY_LIMIT: usize = 2 * 1024 * 1024;
const RUN_LIMIT: Duration = Duration::from_secs(60);
const TOTAL_LIMIT: Duration = Duration::from_secs(110);

struct Workdir(PathBuf);
impl Workdir {
    fn new() -> Result<Self> {
        let path = std::env::temp_dir().join(format!("memhot-web-{}", crate::id()));
        std::fs::create_dir(&path).map_err(|_| Error::internal())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
                .map_err(|_| Error::internal())?;
        }
        Ok(Self(path))
    }
}
impl Drop for Workdir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
#[derive(Clone)]
struct Policy {
    target: Url,
    read: bool,
}
fn operator_endpoint(raw: &str) -> Result<Url> {
    let mut url = Url::parse(raw).map_err(|_| Error::bad("网页搜索地址无效"))?;
    if url.scheme() != "http"
        || url.host_str() != Some("127.0.0.1")
        || url.port().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !matches!(url.path(), "" | "/")
    {
        return Err(Error::bad("托管搜索必须使用带端口的127.0.0.1服务"));
    }
    url.set_path("/search");
    Ok(url)
}
fn paths() -> Result<(PathBuf, PathBuf)> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let node = std::env::var_os("MEMHOT_AGENT_NODE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/opt/homebrew/bin/node"));
    let model_script = std::env::var_os("MEMHOT_AGENT_SCRIPT")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("services/memhot-agent/src/main.mjs"));
    let script = model_script.with_file_name("web-main.mjs");
    for path in [&node, &script] {
        if !path.is_absolute() || !std::fs::metadata(path).is_ok_and(|m| m.is_file()) {
            return Err(Error::upstream(
                "web_plugin_unavailable",
                "Pi网页插件运行时尚未配置",
            ));
        }
    }
    Ok((node, script))
}
pub(crate) fn canonical_read_url(raw: &str) -> Result<Url> {
    let mut url = network::validate_url(raw)?;
    url.set_fragment(None);
    if url.host_str() != Some("github.com") {
        return Ok(url);
    }
    let raw_path = raw
        .split_once("://")
        .map(|(_, rest)| rest)
        .and_then(|rest| rest.split_once('/').map(|(_, path)| path))
        .unwrap_or_default()
        .split(['?', '#'])
        .next()
        .unwrap_or_default();
    let parts: Vec<_> = raw_path.split('/').collect();
    if url.query().is_some()
        || url.port().is_some()
        || parts.len() < 5
        || parts[2] != "blob"
        || parts
            .iter()
            .any(|s| s.is_empty() || matches!(*s, "." | "..") || s.contains(['%', '\\']))
        || parts[..2]
            .iter()
            .chain(std::iter::once(&parts[3]))
            .any(|s| {
                !s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
            })
    {
        return Err(Error::bad(
            "GitHub来源须为无查询参数、明确单层版本的文件链接",
        ));
    }
    let canonical = format!(
        "https://raw.githubusercontent.com/{}/{}/{}/{}",
        parts[0],
        parts[1],
        parts[3],
        parts[4..].join("/")
    );
    network::validate_url(&canonical)
}
fn request(policy: &Policy, value: &Value) -> Result<(Url, Method, HeaderMap, Option<String>)> {
    let mut url = Url::parse(value["url"].as_str().unwrap_or_default())
        .map_err(|_| Error::bad("网页插件请求地址无效"))?;
    url.set_fragment(None);
    let method = match value["method"].as_str() {
        Some("GET") => Method::GET,
        Some("POST") if !policy.read => Method::POST,
        _ => return Err(Error::bad("网页插件请求方法不受支持")),
    };
    if !url.username().is_empty()
        || url.password().is_some()
        || url.as_str().len() > 4096
        || (policy.read && url != policy.target)
        || (!policy.read
            && (url.origin() != policy.target.origin() || url.path() != policy.target.path()))
    {
        return Err(Error::bad("网页插件请求超出授权来源"));
    }
    if policy.read {
        network::validate_url(url.as_str())?;
    }
    let mut headers = HeaderMap::new();
    if let Some(fields) = value["headers"].as_object() {
        for (name, value) in fields {
            if !matches!(
                name.to_ascii_lowercase().as_str(),
                "accept" | "content-type" | "user-agent"
            ) {
                return Err(Error::bad("网页插件请求包含未授权头信息"));
            }
            let value = value
                .as_str()
                .filter(|s| s.len() <= 512)
                .ok_or_else(|| Error::bad("网页插件请求头无效"))?;
            let name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| Error::bad("网页插件请求头无效"))?;
            let value = reqwest::header::HeaderValue::from_str(value)
                .map_err(|_| Error::bad("网页插件请求头无效"))?;
            headers.insert(name, value);
        }
    } else if !value["headers"].is_null() {
        return Err(Error::bad("网页插件请求头格式无效"));
    }
    let body = match value.get("body") {
        Some(Value::String(text)) if text.len() <= 16 * 1024 && method == Method::POST => {
            Some(text.clone())
        }
        None | Some(Value::Null) => None,
        _ => return Err(Error::bad("网页插件请求正文超出权限或大小限制")),
    };
    Ok((url, method, headers, body))
}
// A source's byte budget is recoverable; worker/wire/output overruns stay terminal.
fn read_source_error(error: Error) -> Error {
    if error.code == "response_too_large" {
        Error::upstream(
            "web_text_unavailable",
            "公开来源超出读取大小限制，请选择其他公开来源",
        )
    } else {
        error
    }
}
async fn http(policy: &Policy, value: &Value) -> Result<Value> {
    let (url, method, headers, body) = request(policy, value)?;
    if policy.read {
        // The existing fetcher pins DNS and revalidates every redirect independently.
        let fetched = network::fetch_document(url.as_str())
            .await
            .map_err(read_source_error)?;
        return document_response(fetched);
    }
    let client = if policy.target.host_str() == Some("127.0.0.1") {
        reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(3))
            .timeout(RUN_LIMIT)
            .build()
            .map_err(|_| Error::internal())?
    } else {
        network::client_for(url.as_str()).await?
    };
    let mut req = client.request(method, url.clone()).headers(headers);
    if let Some(body) = body {
        req = req.body(body);
    }
    let response = req.send().await.map_err(network::transport_error)?;
    if !response.status().is_success() {
        // Never pass provider error pages, authentication details or response headers to Node.
        return Err(network::status_error(response.status()));
    }
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .chars()
        .take(128)
        .collect::<String>();
    let body = String::from_utf8(network::bounded(response, BODY_LIMIT).await?)
        .map_err(|_| Error::upstream("invalid_response", "网页搜索不是有效文本"))?;
    Ok(json!({"status":status,"contentType":content_type,"body":body,"url":url.to_string()}))
}
async fn frame<R: AsyncBufRead + Unpin>(reader: &mut R) -> Result<(Value, usize)> {
    let mut bytes = Vec::new();
    loop {
        let buf = reader
            .fill_buf()
            .await
            .map_err(|_| Error::upstream("web_plugin_protocol", "无法读取网页插件输出"))?;
        if buf.is_empty() {
            return Err(Error::upstream(
                "web_plugin_protocol",
                "网页插件在完成前退出",
            ));
        }
        let count = buf
            .iter()
            .position(|b| *b == b'\n')
            .map_or(buf.len(), |i| i + 1);
        if bytes.len() + count > OUTPUT_LIMIT {
            return Err(Error::upstream(
                "response_too_large",
                "网页插件输出超出大小限制",
            ));
        }
        let done = buf[count - 1] == b'\n';
        bytes.extend_from_slice(&buf[..count]);
        reader.consume(count);
        if done {
            break;
        }
    }
    let value = serde_json::from_slice(&bytes)
        .map_err(|_| Error::upstream("web_plugin_protocol", "网页插件输出格式无效"))?;
    Ok((value, bytes.len()))
}
async fn send(
    stdin: &mut tokio::process::ChildStdin,
    value: &Value,
    wire: &mut usize,
) -> Result<()> {
    let mut bytes = serde_json::to_vec(value)?;
    if bytes.len() > INPUT_LIMIT || *wire + bytes.len() > WIRE_LIMIT {
        return Err(Error::upstream(
            "response_too_large",
            "网页插件通信超出大小限制",
        ));
    }
    *wire += bytes.len();
    bytes.push(b'\n');
    stdin
        .write_all(&bytes)
        .await
        .map_err(|_| Error::upstream("web_plugin_protocol", "网页插件输入已关闭"))?;
    stdin.flush().await.map_err(|_| Error::internal())
}
#[path = "agent_web_sources.rs"]
mod sources;
use sources::{document_response, materials, ReadMetadata};
async fn execute(
    node: PathBuf,
    script: PathBuf,
    start: Value,
    policy: Policy,
) -> Result<Vec<Material>> {
    static GATE: std::sync::OnceLock<tokio::sync::Semaphore> = std::sync::OnceLock::new();
    let _permit = GATE
        .get_or_init(|| tokio::sync::Semaphore::new(1))
        .acquire()
        .await
        .map_err(|_| Error::internal())?;
    let workdir = Workdir::new()?;
    let mut child = tokio::process::Command::new(node)
        .arg("--max-old-space-size=128")
        .arg(script)
        .env_clear()
        .env("MEMHOT_WEB_WORKDIR", &workdir.0)
        .env("HOME", &workdir.0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| Error::upstream("web_plugin_unavailable", "无法启动Pi网页插件"))?;
    let mut stdin = child.stdin.take().ok_or_else(Error::internal)?;
    let mut stdout = BufReader::new(child.stdout.take().ok_or_else(Error::internal)?);
    let (mut wire, mut output, mut fetched) = (0, 0, None);
    send(&mut stdin, &start, &mut wire).await?;
    for _ in 0..16 {
        let (value, bytes) = frame(&mut stdout).await?;
        output += bytes;
        wire += bytes;
        if output > OUTPUT_LIMIT || wire > WIRE_LIMIT {
            return Err(Error::upstream(
                "response_too_large",
                "网页插件累计输出超出大小限制",
            ));
        }
        match value["type"].as_str() {
            Some("http") => {
                let id = value["id"]
                    .as_str()
                    .filter(|s| !s.is_empty() && s.len() <= 100)
                    .ok_or_else(|| Error::bad("网页插件帧ID无效"))?;
                let response = match http(&policy, &value).await {
                    Ok(body) => {
                        if policy.read {
                            fetched = ReadMetadata::from_response(&body);
                        }
                        json!({"type":"result","id":id,"value":body})
                    }
                    Err(e) => {
                        send(&mut stdin, &json!({"type":"error","id":id,"code":e.code,"message":"网页来源不可用或请求超出授权，请选择其他公开来源"}), &mut wire).await?;
                        return Err(e);
                    }
                };
                send(&mut stdin, &response, &mut wire).await?;
            }
            Some("complete") => {
                let found = materials(&value, &policy, fetched.as_ref())?;
                drop(stdin);
                let status = tokio::time::timeout(Duration::from_secs(2), child.wait())
                    .await
                    .map_err(|_| Error::upstream("web_plugin_protocol", "网页插件完成后未退出"))?
                    .map_err(|_| Error::internal())?;
                if !status.success() {
                    return Err(Error::upstream("web_plugin_protocol", "网页插件异常退出"));
                }
                return Ok(found);
            }
            Some("error" | "failed") => {
                return Err(Error::upstream(
                    "web_plugin_failed",
                    "Pi网页插件未取得可用公开资料，请稍后重试",
                ))
            }
            _ => {
                return Err(Error::upstream(
                    "web_plugin_protocol",
                    "网页插件返回未知消息",
                ))
            }
        }
    }
    Err(Error::upstream(
        "web_plugin_protocol",
        "网页插件请求次数超出上限",
    ))
}
async fn run(start: Value, policy: Policy) -> Result<Vec<Material>> {
    let (node, script) = paths()?;
    tokio::time::timeout(RUN_LIMIT, execute(node, script, start, policy))
        .await
        .map_err(|_| Error::upstream("timeout", "Pi网页插件达到处理时限"))?
}
pub async fn search(query: &str) -> Result<Vec<Material>> {
    if query.trim().is_empty() || query.chars().count() > 300 {
        return Err(Error::bad("公开搜索词须为1至300字"));
    }
    tokio::time::timeout(TOTAL_LIMIT, async {
        // The plugin's numResults contract trims each response to 20. Prefer our
        // existing operator-owned decoder so every valid SearXNG result reaches
        // the paged candidate pool. Pi still owns reads and fallback discovery.
        if crate::search_backend::configured() {
            if let Ok((found, _warnings)) = crate::search_backend::search(query, false).await {
                if !found.is_empty() { return Ok(found); }
            }
        }
        let degoog = Url::parse("https://degoog.org/api/search").map_err(|_| Error::internal())?;
        let first = run(json!({"type":"start","action":"search","query":query,"provider":"degoog"}), Policy { target: degoog, read: false }).await;
        if let Ok(found) = &first { if !found.is_empty() { return first; } }
        if let Ok(raw) = std::env::var("MEMHOT_SEARCH_URL") {
            let target = operator_endpoint(&raw)?;
            return run(json!({"type":"start","action":"search","query":query,"provider":"searxng","endpoint":raw}), Policy { target, read: false }).await;
        }
        first
    }).await.map_err(|_| Error::upstream("timeout", "公开网页搜索达到处理时限"))?
}
pub async fn read(raw: &str) -> Result<Material> {
    let target = canonical_read_url(raw)?;
    let mut found = run(
        json!({"type":"start","action":"read","url":target.as_str()}),
        Policy { target, read: true },
    )
    .await?;
    if found.len() != 1 || found[0].text.trim().is_empty() {
        return Err(Error::upstream(
            "web_text_unavailable",
            "当前公开网页没有可读取正文，请选择其他来源",
        ));
    }
    Ok(found.remove(0))
}

#[cfg(test)]
#[path = "agent_web_tests.rs"]
mod tests;
