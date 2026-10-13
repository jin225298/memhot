//! Search providers return discovery snippets, never proof of full-text reading.
//! Wire schemas checked 2026-10-13 against:
//! https://docs.tavily.com/documentation/api-reference/endpoint/search
//! https://exa.ai/docs/reference/search
use crate::{
    clip,
    discovery::Material,
    error::{Error, Result},
    freshness, network, search_backend,
    subscription_preview::public_url,
};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};
use serde_json::{json, Value};
use std::{collections::HashSet, sync::OnceLock, time::Duration};
const MAX_RESULTS: usize = 20; // Tavily public API per-request maximum; not a run limit.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
#[derive(Clone, Copy)]
pub struct Query<'a> {
    pub text: &'a str,
    pub recent: bool,
}
#[derive(Default)]
pub struct SearchOutput {
    pub materials: Vec<Material>,
    pub warnings: Vec<String>,
    /// Caller may try public scholarly/discovery APIs once. This module never does.
    pub needs_public_fallback: bool,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Paid {
    Tavily,
    Exa,
}
impl Paid {
    fn endpoint(self) -> &'static str {
        match self {
            Self::Tavily => "https://api.tavily.com/search",
            Self::Exa => "https://api.exa.ai/search",
        }
    }
    fn name(self) -> &'static str {
        match self {
            Self::Tavily => "Tavily",
            Self::Exa => "Exa",
        }
    }
}
fn selected(config: &Value) -> Option<Paid> {
    match config["provider"].as_str() {
        Some("tavily") => Some(Paid::Tavily),
        Some("exa") => Some(Paid::Exa),
        _ => None,
    }
}
pub fn paid_configured(config: &Value) -> bool {
    selected(config).is_some()
        && config["apiKey"]
            .as_str()
            .is_some_and(|s| !s.trim().is_empty())
}
fn request(provider: Paid, key: &str, query: Query<'_>) -> Result<(HeaderMap, Value)> {
    if key.trim().is_empty() || key.len() > 8192 {
        return Err(Error::bad("搜索密钥无效"));
    }
    let mut headers = HeaderMap::new();
    let (name, value) = if provider == Paid::Tavily {
        (AUTHORIZATION, format!("Bearer {}", key.trim()))
    } else {
        (
            reqwest::header::HeaderName::from_static("x-api-key"),
            key.trim().to_owned(),
        )
    };
    let mut value = HeaderValue::from_str(&value).map_err(|_| Error::bad("搜索密钥无效"))?;
    value.set_sensitive(true);
    headers.insert(name, value);
    let text = clip(query.text.trim(), 300);
    let body = match provider {
        Paid::Tavily => {
            let mut body = json!({"query":text,"max_results":MAX_RESULTS,"search_depth":"basic","topic":"general","include_answer":false,"include_raw_content":false,"include_published_date":true,"auto_parameters":false});
            // Provider date filters are discovery hints; original-page dates remain authoritative.
            if query.recent {
                body["time_range"] = json!("week");
            }
            body
        }
        Paid::Exa => {
            json!({"query":if query.recent { format!("Recent developments: {text}") } else { text },"type":"auto","numResults":MAX_RESULTS,"contents":{"highlights":true}})
        }
    };
    Ok((headers, body))
}
fn malformed(provider: Paid) -> Error {
    Error::upstream(
        "invalid_response",
        format!("{} 搜索返回了无效结果", provider.name()),
    )
}
fn decode(provider: Paid, value: &Value, key: &str) -> Result<Vec<Material>> {
    if !value["error"].is_null() || !value["detail"].is_null() {
        return Err(malformed(provider));
    }
    let results = value["results"]
        .as_array()
        .ok_or_else(|| malformed(provider))?;
    let mut materials = Vec::new();
    let mut seen = HashSet::new();
    for result in results.iter() {
        let Some(raw) = result["url"].as_str() else {
            continue;
        };
        let Ok(url) = public_url(raw) else {
            continue;
        };
        if (!key.is_empty() && raw.contains(key))
            || !seen.insert(freshness::canonical(url.as_str()))
        {
            continue;
        }
        let title = result["title"].as_str().unwrap_or("公开搜索结果");
        let text = match provider {
            Paid::Tavily => result["content"].as_str().unwrap_or_default().to_owned(),
            Paid::Exa => result["highlights"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .take(5)
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default(),
        };
        if text.trim().is_empty()
            || (!key.is_empty() && (text.contains(key) || title.contains(key)))
        {
            continue;
        }
        let date = result[if provider == Paid::Tavily {
            "published_date"
        } else {
            "publishedDate"
        }]
        .as_str()
        .and_then(freshness::publication)
        .map(|d| d.to_rfc3339());
        materials.push(Material {
            title: clip(title, 200),
            url: url.to_string(),
            text: clip(&text, 2400),
            published: date,
            discovered: None,
            discovered_via: Some(provider.endpoint().into()),
            publication_source: "search_reported_publication",
            publisher: url.host_str().unwrap_or_default().to_owned(),
            fulltext: false,
            pdf: None,
        });
    }
    if !results.is_empty() && materials.is_empty() {
        return Err(malformed(provider));
    }
    Ok(materials)
}
async fn deadline<T>(future: impl std::future::Future<Output = Result<T>>) -> Result<T> {
    tokio::time::timeout(REQUEST_TIMEOUT, future)
        .await
        .map_err(|_| Error::upstream("timeout", "搜索服务响应超时"))?
}
async fn paid_search(provider: Paid, key: &str, query: Query<'_>) -> Result<Vec<Material>> {
    let (headers, body) = request(provider, key, query)?;
    let value = deadline(network::json_post(provider.endpoint(), headers, body)).await?;
    decode(provider, &value, key.trim())
}
fn merge(output: &mut SearchOutput, provider: &str, result: Result<Vec<Material>>) {
    match result {
        Ok(materials) if !materials.is_empty() => output.materials.extend(materials),
        Ok(_) => {
            output
                .warnings
                .push(format!("{provider} 未找到可用结果，可继续使用公开搜索"));
            output.needs_public_fallback = true;
        }
        Err(error) => {
            // Only our fixed code/category is rendered: upstream errors may echo credentials.
            let reason = match error.code {
                "timeout" => "响应超时",
                "provider_auth" => "密钥无效或权限不足",
                "rate_limited" => "暂时限流",
                "invalid_response" => "结果格式无效",
                _ => "暂时不可用",
            };
            output
                .warnings
                .push(format!("{provider} {reason}，可继续使用公开搜索"));
            output.needs_public_fallback = true;
        }
    }
}
fn balance(materials: &mut Vec<Material>, free_count: usize) {
    let paid = materials.split_off(free_count);
    let free = std::mem::take(materials);
    let mut free = free.into_iter();
    let mut paid = paid.into_iter();
    loop {
        let pair = (free.next(), paid.next());
        if pair.0.is_none() && pair.1.is_none() {
            break;
        }
        materials.extend(pair.0);
        materials.extend(pair.1);
    }
}
/// Runs each configured provider once; free public APIs and Pi's installed web plugin
/// remain caller-controlled so a paid failure cannot silently replace free search.
pub async fn configured_search(config: &Value, text: &str, recent: bool) -> SearchOutput {
    let mut output = SearchOutput::default();
    if text.trim().is_empty() {
        output.warnings.push("搜索问题不能为空".into());
        return output;
    }
    static GATE: OnceLock<tokio::sync::Semaphore> = OnceLock::new();
    let Ok(_permit) = GATE
        .get_or_init(|| tokio::sync::Semaphore::new(4))
        .try_acquire()
    else {
        output
            .warnings
            .push("搜索正在处理其他请求，可继续使用公开搜索".into());
        output.needs_public_fallback = true;
        return output;
    };
    let query = Query { text, recent };
    let provider = selected(config);
    let key = config["apiKey"].as_str().unwrap_or_default();
    let free = async {
        if !search_backend::configured() {
            return None;
        }
        Some(deadline(search_backend::search(text, recent)).await)
    };
    let paid = async {
        match provider {
            Some(provider) => Some(paid_search(provider, key, query).await),
            None => None,
        }
    };
    let (free, paid) = tokio::join!(free, paid);
    if let Some(result) = free {
        let result = result.map(|(mut materials, warnings)| {
            output.warnings.extend(warnings);
            materials.retain(|m| {
                public_url(&m.url).is_ok()
                    && (key.is_empty()
                        || (!m.url.contains(key)
                            && !m.title.contains(key)
                            && !m.text.contains(key)))
            });
            for material in &mut materials {
                if let Ok(url) = public_url(&material.url) {
                    material.url = url.to_string();
                }
                material.published = material
                    .published
                    .as_deref()
                    .and_then(freshness::publication)
                    .map(|d| d.to_rfc3339());
            }
            materials
        });
        merge(&mut output, "SearXNG", result);
    }
    let free_count = output.materials.len();
    if let Some(result) = paid {
        merge(&mut output, provider.unwrap().name(), result);
    }
    // Interleave providers for the first page while retaining every unique result.
    // Subsequent pages remain available for the model to compare and select.
    balance(&mut output.materials, free_count);
    if provider.is_none() && !matches!(config["provider"].as_str(), None | Some("public")) {
        output
            .warnings
            .push("搜索服务配置无效，可继续使用公开搜索".into());
        output.needs_public_fallback = true;
    }
    let mut seen = HashSet::new();
    output
        .materials
        .retain(|m| seen.insert(freshness::canonical(&m.url)));
    output.needs_public_fallback |= output.materials.is_empty();
    output
}
#[cfg(test)]
#[path = "search_providers_tests.rs"]
mod tests;
