//! Free metasearch is an operator-owned loopback service, never a user-supplied proxy.
use crate::{
    clip,
    discovery::Material,
    error::{Error, Result},
    network,
};
use serde_json::Value;
use std::{sync::OnceLock, time::Duration};

fn endpoint(raw: &str) -> Result<reqwest::Url> {
    let mut url = reqwest::Url::parse(raw).map_err(|_| Error::bad("搜索服务地址无效"))?;
    if url.scheme() != "http"
        || url.host_str() != Some("127.0.0.1")
        || url.port().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !matches!(url.path(), "" | "/")
    {
        return Err(Error::bad("托管搜索必须配置为带端口的127.0.0.1 HTTP服务"));
    }
    url.set_path("/search");
    Ok(url)
}
pub fn configured() -> bool {
    std::env::var_os("MEMHOT_SEARCH_URL").is_some()
}
pub async fn search(query: &str, recent: bool) -> Result<(Vec<Material>, Vec<String>)> {
    let raw = std::env::var("MEMHOT_SEARCH_URL").map_err(|_| Error::bad("免费聚合搜索尚未配置"))?;
    let url = endpoint(&raw)?;
    static GATE: OnceLock<tokio::sync::Semaphore> = OnceLock::new();
    let _permit = tokio::time::timeout(
        Duration::from_secs(10),
        GATE.get_or_init(|| tokio::sync::Semaphore::new(2))
            .acquire(),
    )
    .await
    .map_err(|_| Error::upstream("busy", "免费搜索正在处理其他请求"))?
    .map_err(|_| Error::internal())?;
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|_| Error::internal())?;
    let short = clip(query, 300);
    let mut fields = vec![
        ("q", short.as_str()),
        ("format", "json"),
        ("categories", "general"),
        ("language", "auto"),
        ("safesearch", "1"),
    ];
    // Engine date filters are hints. Actual publication dates are checked after retrieval.
    if recent {
        fields.push(("time_range", "month"));
    }
    let response = client
        .post(url)
        .form(&fields)
        .send()
        .await
        .map_err(network::transport_error)?;
    if !response.status().is_success() {
        return Err(network::status_error(response.status()));
    }
    let value: Value = serde_json::from_slice(&network::bounded(response, 1024 * 1024).await?)
        .map_err(|_| Error::upstream("invalid_response", "免费搜索返回了无法读取的数据"))?;
    decode(&value)
}
fn decode(value: &Value) -> Result<(Vec<Material>, Vec<String>)> {
    let results = value["results"]
        .as_array()
        .ok_or_else(|| Error::upstream("invalid_response", "免费搜索缺少结果列表"))?;
    let mut materials = Vec::new();
    for result in results.iter().take(12) {
        let Some(raw) = result["url"].as_str() else {
            continue;
        };
        let Ok(url) = network::validate_url(raw) else {
            continue;
        };
        if url.query().is_some_and(|q| q.len() > 1000) {
            continue;
        }
        materials.push(Material {
            title: clip(result["title"].as_str().unwrap_or("公开搜索结果"), 200),
            url: url.to_string(),
            text: clip(result["content"].as_str().unwrap_or_default(), 2400),
            published: result["publishedDate"].as_str().map(str::to_owned),
            discovered: None,
            discovered_via: Some("SearXNG（发现渠道）".into()),
            publication_source: "search_reported_publication",
            publisher: url.host_str().unwrap_or_default().to_owned(),
            fulltext: false,
            pdf: None,
        });
    }
    let mut warnings = Vec::new();
    if value["unresponsive_engines"]
        .as_array()
        .is_some_and(|a| !a.is_empty())
    {
        warnings.push("部分免费搜索来源限流或暂时不可用，结果可能不完整".into());
    }
    if materials.is_empty() {
        warnings.push("免费聚合搜索未返回可验证的公开结果".into());
    }
    Ok((materials, warnings))
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn operator_search_endpoint_cannot_become_an_open_ssrf_proxy() {
        assert!(endpoint("http://127.0.0.1:8791").is_ok());
        for raw in [
            "http://127.0.0.1",
            "http://localhost:8791",
            "http://example.org:80",
            "http://127.0.0.1:8791/private",
            "http://user:password@127.0.0.1:8791",
            "http://127.0.0.1:8791/?q=private",
        ] {
            assert!(endpoint(raw).is_err(), "{raw}");
        }
    }
    #[test]
    fn snippets_stay_snippets_and_outages_are_not_silently_successful() {
        let (found,warnings)=decode(&json!({"results":[{"url":"http://127.0.0.1/private","title":"private"},{"url":"https://example.org/paper","title":"Paper","content":"An indexed excerpt","publishedDate":"2026-10-10"}],"unresponsive_engines":[["engine","timeout"]]})).unwrap();
        assert_eq!(found.len(), 1);
        assert!(!found[0].fulltext);
        assert_eq!(warnings.len(), 1);
        assert_eq!(found[0].publication_source, "search_reported_publication");
        assert!(decode(&json!({"error":"private body"})).is_err());
    }
}
