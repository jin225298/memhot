//! Bounded, public-network RSS/Atom discovery. Preview never creates a subscription.
use crate::{
    clip,
    discovery::plain,
    error::{Error, Result},
    feed, network,
};
use quick_xml::{events::Event, Reader};
use reqwest::Url;
use serde_json::{json, Value};
use std::{collections::HashSet, time::Duration};
const MAX_FEEDS: usize = 5;
const MAX_DEPTH: usize = 64;

pub fn public_url(raw: &str) -> Result<Url> {
    if raw.len() > 4096 {
        return Err(Error::bad("订阅网址过长"));
    }
    let mut url = network::validate_url(raw.trim())?;
    for (key, _) in url.query_pairs() {
        let key = key.to_ascii_lowercase().replace(['-', '_'], "");
        if [
            "token",
            "secret",
            "password",
            "passwd",
            "credential",
            "signature",
            "authorization",
            "authkey",
            "authentication",
            "apikey",
            "accesskey",
            "session",
            "cookie",
        ]
        .iter()
        .any(|part| key.contains(part))
            || matches!(key.as_str(), "key" | "auth" | "sig" | "code")
        {
            return Err(Error::bad(
                "此预览仅支持公开 RSS，请移除含密钥或登录凭据的网址参数",
            ));
        }
    }
    url.set_fragment(None);
    Ok(url)
}

async fn fetch(raw: &str) -> Result<network::Fetched> {
    tokio::time::timeout(Duration::from_secs(4), async {
        let mut url = public_url(raw)?;
        for hop in 0..=4 {
            // Existing transport pins validated public DNS answers and disables auto redirects.
            let response = network::client_for(url.as_str())
                .await?
                .get(url.clone())
                .send()
                .await
                .map_err(network::transport_error)?;
            if response.status().is_redirection() {
                if hop == 4 {
                    return Err(Error::bad("来源重定向过多"));
                }
                let location = response
                    .headers()
                    .get("location")
                    .and_then(|h| h.to_str().ok())
                    .ok_or_else(|| Error::bad("重定向地址无效"))?;
                url = public_url(
                    url.join(location)
                        .map_err(|_| Error::bad("重定向地址无效"))?
                        .as_str(),
                )?;
                continue;
            }
            if !response.status().is_success() {
                return Err(network::status_error(response.status()));
            }
            let content_type = response
                .headers()
                .get("content-type")
                .and_then(|h| h.to_str().ok())
                .unwrap_or("")
                .to_owned();
            let text = String::from_utf8(network::bounded(response, network::MAX_BYTES).await?)
                .map_err(|_| Error::bad("来源不是有效 UTF-8 文本"))?;
            return Ok(network::Fetched {
                url: url.to_string(),
                text,
                content_type,
            });
        }
        Err(Error::internal())
    })
    .await
    .map_err(|_| Error::upstream("timeout", "订阅预览超时，请稍后重试"))?
}

#[derive(Default)]
struct FeedMetadata {
    title: String,
    description: String,
    entries: usize,
}
fn invalid_feed() -> Error {
    Error::bad("来源不是完整有效的 RSS 或 Atom 订阅")
}
// Validate the entire XML before using the established feed parser (which tolerates
// partial feeds for discovery). Empty feeds succeed only after structural validation.
fn metadata(xml: &str) -> Result<FeedMetadata> {
    if xml.len() > network::MAX_BYTES {
        return Err(Error::bad("订阅文件超出大小限制"));
    }
    let mut reader = Reader::from_str(xml);
    reader.config_mut().expand_empty_elements = true;
    let mut stack = Vec::<String>::new();
    let mut root = None;
    let mut closed = false;
    let mut channel = false;
    let mut meta = FeedMetadata::default();
    loop {
        match reader.read_event().map_err(|_| invalid_feed())? {
            Event::Start(e) => {
                let name = String::from_utf8_lossy(e.local_name().as_ref()).to_string();
                if stack.is_empty() {
                    if closed || root.is_some() || !matches!(name.as_str(), "rss" | "feed" | "RDF")
                    {
                        return Err(invalid_feed());
                    }
                    root = Some(name.clone());
                }
                for attribute in e.attributes() {
                    attribute
                        .map_err(|_| invalid_feed())?
                        .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                        .map_err(|_| invalid_feed())?;
                }
                if name == "channel" && stack.len() == 1 {
                    channel = true;
                }
                if matches!(name.as_str(), "entry" | "item") {
                    meta.entries += 1;
                    if meta.entries > 10_000 {
                        return Err(Error::bad("订阅条目过多"));
                    }
                }
                stack.push(name);
                if stack.len() > MAX_DEPTH {
                    return Err(Error::bad("订阅文件嵌套过深"));
                }
            }
            Event::End(_) => {
                stack.pop().ok_or_else(invalid_feed)?;
                if stack.is_empty() {
                    closed = true;
                }
            }
            Event::Text(t) => {
                let text = t.decode().map_err(|_| invalid_feed())?;
                if stack.is_empty() && !text.trim().is_empty() {
                    return Err(invalid_feed());
                }
                append_metadata(&mut meta, &stack, &text);
            }
            Event::CData(t) => {
                if stack.is_empty() {
                    return Err(invalid_feed());
                }
                append_metadata(&mut meta, &stack, &t.decode().map_err(|_| invalid_feed())?);
            }
            Event::GeneralRef(e) => {
                let name = e.decode().map_err(|_| invalid_feed())?;
                let escaped = format!("&{name};");
                let text = quick_xml::escape::unescape(&escaped).map_err(|_| invalid_feed())?;
                if stack.is_empty() {
                    return Err(invalid_feed());
                }
                append_metadata(&mut meta, &stack, &text);
            }
            Event::DocType(_) => return Err(Error::bad("订阅 XML 不允许 DTD 或自定义实体")),
            Event::Eof => break,
            _ => {}
        }
    }
    if !closed || !stack.is_empty() || (root.as_deref() != Some("feed") && !channel) {
        return Err(invalid_feed());
    }
    Ok(meta)
}
fn append_metadata(meta: &mut FeedMetadata, stack: &[String], text: &str) {
    let field = if stack.len() == 2 && stack[0] == "feed" {
        stack.last()
    } else if stack.len() == 3 && stack[1] == "channel" {
        stack.last()
    } else {
        None
    };
    match field.map(String::as_str) {
        Some("title") => meta.title.push_str(text),
        Some("description" | "subtitle") => meta.description.push_str(text),
        _ => {}
    }
}
pub fn parse_feed(xml: &str, url: &str) -> Result<Vec<crate::discovery::Material>> {
    metadata(xml)?;
    Ok(feed::parse_from(xml, Some(url)))
}
fn parse_candidate(xml: &str, url: &str) -> Result<Value> {
    let meta = metadata(xml)?;
    let items: Vec<_> = feed::parse_from(xml, Some(url))
        .into_iter()
        .filter(|item| public_url(&item.url).is_ok())
        .take(3)
        .map(|item| {
            json!({
                "title": clip(&item.title, 300), "url": public_url(&item.url).unwrap().to_string(),
                "publishedAt": item.published.as_deref().and_then(crate::freshness::publication).map(|date| date.to_rfc3339()), "summary": clip(&item.text, 500)
            })
        })
        .collect();
    if meta.entries > 0 && items.is_empty() {
        return Err(Error::bad("订阅包含条目，但未找到可用的公开文章链接"));
    }
    let coverage = if items.is_empty() {
        "empty"
    } else if items
        .iter()
        .any(|i| i["summary"].as_str().is_some_and(|s| !s.is_empty()))
    {
        "summary"
    } else {
        "title_only"
    };
    let title = clip(&plain(&format!("<body>{}</body>", meta.title)), 120);
    Ok(
        json!({"url": url, "title": if title.is_empty() { public_url(url)?.host_str().unwrap_or("RSS").to_owned() } else { title },
        "description": clip(&plain(&format!("<body>{}</body>", meta.description)), 500), "items": items, "coverage": coverage}),
    )
}
fn discover(html: &str, base: &str) -> Result<(Vec<(String, String)>, Vec<String>)> {
    let base = public_url(base)?;
    let doc = dom_query::Document::from(html);
    let mut seen = HashSet::new();
    let mut links = Vec::new();
    let mut warnings = Vec::new();
    for link in doc.select("link[href]").iter() {
        let rel = link.attr("rel").unwrap_or_default();
        let mime = link.attr("type").unwrap_or_default().to_ascii_lowercase();
        if !rel
            .split_ascii_whitespace()
            .any(|r| r.eq_ignore_ascii_case("alternate"))
            || !matches!(
                mime.split(';').next().unwrap_or("").trim(),
                "application/rss+xml" | "application/atom+xml" | "application/rdf+xml"
            )
        {
            continue;
        }
        let href = link.attr("href").unwrap_or_default();
        let url = base
            .join(&href)
            .map_err(|_| Error::bad("页面含无效订阅链接"))
            .and_then(|url| public_url(url.as_str()));
        let Ok(url) = url else {
            if !warnings
                .iter()
                .any(|w| w == "已跳过不安全或含凭据的订阅链接")
            {
                warnings.push("已跳过不安全或含凭据的订阅链接".into());
            }
            continue;
        };
        if !seen.insert(url.to_string()) {
            continue;
        }
        if links.len() == MAX_FEEDS {
            warnings.push("页面订阅较多，仅预览前 5 个来源".into());
            break;
        }
        links.push((
            url.to_string(),
            clip(&link.attr("title").unwrap_or_default(), 120),
        ));
    }
    Ok((links, warnings))
}
pub async fn preview(raw: &str) -> Result<Value> {
    tokio::time::timeout(Duration::from_secs(24), preview_inner(raw))
        .await
        .map_err(|_| Error::upstream("timeout", "订阅预览超时，请稍后重试"))?
}
async fn preview_inner(raw: &str) -> Result<Value> {
    let fetched = fetch(raw).await?;
    match parse_candidate(&fetched.text, &fetched.url) {
        Ok(candidate) => return Ok(json!({"candidates": [candidate], "warnings": []})),
        Err(error) => {
            // RSS/Atom that fails validation must not be reported as successful HTML discovery.
            let mime = fetched.content_type.to_ascii_lowercase();
            let start = fetched.text.trim_start_matches('\u{feff}').trim_start();
            if mime.contains("xml")
                || start.starts_with("<?xml")
                || start.starts_with("<rss")
                || start.starts_with("<feed")
            {
                return Err(error);
            }
        }
    }
    let (links, warnings) = discover(&fetched.text, &fetched.url)?;
    if links.is_empty() {
        return Err(Error::bad("未发现公开 RSS 或 Atom 订阅，请粘贴订阅源地址"));
    }
    let mut candidates = Vec::new();
    for (url, title) in links {
        let result = match fetch(&url).await {
            Ok(feed) => parse_candidate(&feed.text, &feed.url),
            Err(error) => Err(error),
        };
        candidates.push(result.unwrap_or_else(|error| json!({"url": url, "title": title, "description": "", "items": [], "coverage": "unavailable", "error": {"code": error.code, "message": error.message}})));
    }
    Ok(json!({"candidates": candidates, "warnings": warnings}))
}
#[cfg(test)]
#[path = "subscription_preview_tests.rs"]
mod tests;
