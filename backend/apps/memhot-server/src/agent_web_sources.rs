//! Host-authenticated source identity and original-page publication metadata.
use super::{Material, Policy, OUTPUT_LIMIT};
use crate::{
    discovery::PdfExtraction,
    error::{Error, Result},
    network,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};

pub(super) fn document_response(fetched: network::FetchedDocument) -> Result<Value> {
    let (body, content_type, encoding) = if fetched.pdf {
        (
            STANDARD.encode(fetched.data),
            "application/pdf".to_owned(),
            "base64",
        )
    } else {
        (
            String::from_utf8(fetched.data).map_err(|_| Error::bad("来源不是有效UTF-8文本"))?,
            fetched.content_type.chars().take(128).collect(),
            "utf8",
        )
    };
    Ok(
        json!({"status":200,"contentType":content_type,"body":body,"bodyEncoding":encoding,"url":fetched.url}),
    )
}

pub(super) struct ReadMetadata {
    pub(super) url: String,
    pub(super) published: Option<String>,
    pub(super) pdf: bool,
}
impl ReadMetadata {
    pub(super) fn from_response(response: &Value) -> Option<Self> {
        let content_type = response["contentType"]
            .as_str()
            .unwrap_or_default()
            .split(';')
            .next()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        Some(Self {
            url: response["url"].as_str()?.to_owned(),
            pdf: content_type == "application/pdf",
            // Read the host-fetched original HTML before the plugin strips metadata.
            // Worker output and retrieval timestamps cannot establish publication.
            published: if matches!(content_type.as_str(), "text/html" | "application/xhtml+xml") {
                crate::freshness::page_publication(
                    response["url"].as_str()?,
                    response["body"].as_str()?,
                )
            } else {
                None
            },
        })
    }
}
pub(super) fn materials(
    value: &Value,
    policy: &Policy,
    fetched: Option<&ReadMetadata>,
) -> Result<Vec<Material>> {
    let sources = value["sources"]
        .as_array()
        .filter(|v| v.len() <= 12)
        .ok_or_else(|| Error::upstream("web_plugin_protocol", "网页插件来源列表无效"))?;
    let mut result = Vec::new();
    for source in sources {
        let url = network::validate_url(source["url"].as_str().unwrap_or_default())?;
        if policy.read && Some(url.as_str()) != fetched.map(|page| page.url.as_str()) {
            return Err(Error::bad("网页插件返回了未读取的来源"));
        }
        let text = source["text"]
            .as_str()
            .filter(|s| s.len() <= OUTPUT_LIMIT)
            .ok_or_else(|| Error::upstream("web_plugin_protocol", "网页插件正文无效"))?;
        if policy.read && (source["fulltext"] != true || text.trim().chars().count() < 100) {
            return Err(Error::upstream(
                "web_text_unavailable",
                "当前公开网页没有足够可读取正文，请选择其他来源",
            ));
        }
        let pdf = if fetched.is_some_and(|page| page.pdf) {
            Some(
                PdfExtraction::from_value(&source["extraction"]).ok_or_else(|| {
                    Error::upstream("web_plugin_protocol", "网页插件PDF提取范围无效")
                })?,
            )
        } else {
            if !source["extraction"].is_null() {
                return Err(Error::bad("网页插件返回未读取的PDF来源"));
            }
            None
        };
        result.push(Material {
            title: crate::clip(source["title"].as_str().unwrap_or("公开网页"), 200),
            url: url.to_string(),
            text: crate::clip(text, 18000),
            published: fetched
                .filter(|_| policy.read)
                .and_then(|page| page.published.clone()),
            discovered: Some(crate::now()),
            discovered_via: Some("Pi pi-web-access（发现渠道）".into()),
            publication_source: if !policy.read {
                "search_reported_publication"
            } else if fetched.is_some_and(|page| page.published.is_some()) {
                "original_page_metadata"
            } else {
                "unknown"
            },
            publisher: url.host_str().unwrap_or_default().to_owned(),
            fulltext: policy.read && source["fulltext"] == true,
            pdf,
        });
    }
    Ok(result)
}
