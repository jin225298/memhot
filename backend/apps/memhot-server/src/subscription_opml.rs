//! OPML preview is a local parse only: it never fetches or subscribes to entries.
use crate::{
    clip,
    error::{Error, Result},
    subscription_preview::public_url,
};
use quick_xml::{events::Event, Reader};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
pub const MAX_XML_BYTES: usize = 256 * 1024;
const MAX_OUTLINES: usize = 500;
const MAX_DEPTH: usize = 24;
fn invalid() -> Error {
    Error::bad("OPML 文件格式无效或不完整")
}
pub fn preview(xml: &str) -> Result<Value> {
    if xml.len() > MAX_XML_BYTES {
        return Err(Error::bad("OPML 文件不得超过 256 KiB"));
    }
    let mut reader = Reader::from_str(xml);
    reader.config_mut().expand_empty_elements = true;
    let mut stack = Vec::<String>::new();
    let mut folders = Vec::<String>::new();
    let mut feeds = Vec::new();
    let mut rejected = Vec::new();
    let mut seen = HashSet::new();
    let mut duplicates = 0;
    let mut count = 0;
    let mut root_seen = false;
    let mut body_seen = false;
    let mut closed = false;
    loop {
        match reader.read_event().map_err(|_| invalid())? {
            Event::Start(e) => {
                let name = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
                if stack.is_empty() {
                    if root_seen || name != "opml" {
                        return Err(invalid());
                    }
                    root_seen = true;
                }
                let mut attrs = HashMap::new();
                for attribute in e.attributes() {
                    let attr = attribute.map_err(|_| invalid())?;
                    let value = attr
                        .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                        .map_err(|_| invalid())?
                        .into_owned();
                    attrs.insert(
                        String::from_utf8_lossy(attr.key.as_ref()).into_owned(),
                        value,
                    );
                }
                if name == "body" {
                    if stack != ["opml"] || body_seen {
                        return Err(invalid());
                    }
                    body_seen = true;
                }
                if name == "outline" {
                    if !stack.iter().any(|n| n == "body")
                        || !matches!(stack.last().map(String::as_str), Some("body" | "outline"))
                    {
                        return Err(invalid());
                    }
                    count += 1;
                    if count > MAX_OUTLINES {
                        return Err(Error::bad("OPML 最多包含 500 个订阅或文件夹"));
                    }
                    let title = clip(
                        attrs
                            .get("title")
                            .or_else(|| attrs.get("text"))
                            .map(String::as_str)
                            .unwrap_or("")
                            .trim(),
                        120,
                    );
                    if let Some(raw_url) = attrs.get("xmlUrl") {
                        match public_url(raw_url) {
                            Ok(url) => {
                                if !seen.insert(url.to_string()) {
                                    duplicates += 1;
                                } else if feeds.len() >= 100 {
                                    rejected.push(json!({"title":title,"reason":"每次最多导入100个来源，请拆分订阅列表"}));
                                } else {
                                    feeds.push(json!({"url": url.to_string(), "title": if title.is_empty() { url.host_str().unwrap_or("RSS").to_owned() } else { title }, "category": clip(&folders.iter().filter(|s| !s.is_empty()).cloned().collect::<Vec<_>>().join(" / "),80)}));
                                }
                            }
                            Err(error) => {
                                rejected.push(json!({"title": title, "reason": error.message}))
                            }
                        }
                        folders.push(String::new());
                    } else {
                        if attrs
                            .get("type")
                            .is_some_and(|t| t.eq_ignore_ascii_case("rss"))
                        {
                            rejected
                                .push(json!({"title": title, "reason": "订阅缺少 xmlUrl 地址"}));
                        }
                        folders.push(title);
                    }
                }
                stack.push(name);
                if stack.len() > MAX_DEPTH {
                    return Err(Error::bad("OPML 文件夹嵌套过深"));
                }
            }
            Event::End(e) => {
                if e.local_name().as_ref() == b"outline" {
                    folders.pop().ok_or_else(invalid)?;
                }
                stack.pop().ok_or_else(invalid)?;
                if stack.is_empty() {
                    closed = true;
                }
            }
            Event::DocType(_) => return Err(Error::bad("OPML 不允许 DTD 或自定义实体")),
            Event::GeneralRef(e) => {
                let name = e.decode().map_err(|_| invalid())?;
                quick_xml::escape::unescape(&format!("&{name};")).map_err(|_| invalid())?;
                if stack.is_empty() {
                    return Err(invalid());
                }
            }
            Event::Text(e) => {
                if stack.is_empty() && !e.decode().map_err(|_| invalid())?.trim().is_empty() {
                    return Err(invalid());
                }
            }
            Event::CData(_) if stack.is_empty() => return Err(invalid()),
            Event::Eof => break,
            _ => {}
        }
    }
    if !root_seen || !body_seen || !closed || !stack.is_empty() {
        return Err(invalid());
    }
    Ok(json!({"feeds": feeds, "rejected": rejected, "duplicateCount": duplicates}))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn folders_entities_duplicates_and_rejected_urls_are_preserved() {
        let xml = r#"<opml version="2.0"><body><outline text="商业 &amp; 科技"><outline text="分析"><outline title="A &amp; B" xmlUrl="https://example.com/feed?a=1&amp;b=2"/><outline text="duplicate" xmlUrl="https://example.com/feed?a=1&amp;b=2#fragment"/><outline text="private" xmlUrl="http://127.0.0.1/feed"/><outline text="secret" xmlUrl="https://example.com/rss?token=hidden"/></outline></outline></body></opml>"#;
        let result = preview(xml).unwrap();
        assert_eq!(result["feeds"][0]["category"], "商业 & 科技 / 分析");
        assert_eq!(result["feeds"][0]["title"], "A & B");
        assert_eq!(result["duplicateCount"], 1);
        assert_eq!(result["rejected"].as_array().unwrap().len(), 2);
        assert!(!result.to_string().contains("hidden"));
    }
    #[test]
    fn hostile_and_malformed_xml_cannot_be_partial_success() {
        for xml in [
            "<opml><body>",
            "<html/>",
            "<opml/>",
            "<opml><body/></opml><opml/>",
            "<!DOCTYPE opml [<!ENTITY xx SYSTEM 'file:///etc/passwd'>]><opml><body/></opml>",
            "<opml><body><outline text='&unknown;' /></body></opml>",
            "<opml><body></opml>",
        ] {
            assert!(preview(xml).is_err(), "{xml}");
        }
        assert!(preview("<opml><body/></opml>").unwrap()["feeds"]
            .as_array()
            .unwrap()
            .is_empty());
    }
    #[test]
    fn size_outline_and_depth_limits_reject_entire_import() {
        assert!(preview(&" ".repeat(MAX_XML_BYTES + 1)).is_err());
        assert!(preview(&format!(
            "<opml><body>{}</body></opml>",
            "<outline/>".repeat(MAX_OUTLINES + 1)
        ))
        .is_err());
        assert!(preview(&format!(
            "<opml><body>{}{}</body></opml>",
            "<outline>".repeat(MAX_DEPTH),
            "</outline>".repeat(MAX_DEPTH)
        ))
        .is_err());
    }
}

#[cfg(test)]
#[test]
fn preview_is_compatible_with_import_limits() {
    let items = (0..101)
        .map(|n| {
            format!(
                "<outline title='{}' xmlUrl='https://example.com/{n}'/>",
                "标".repeat(180)
            )
        })
        .collect::<String>();
    let xml = format!(
        "<opml><body><outline text='{}'>{items}</outline></body></opml>",
        "分类".repeat(60)
    );
    let result = preview(&xml).unwrap();
    let feeds = result["feeds"].as_array().unwrap();
    assert_eq!(feeds.len(), 100);
    assert_eq!(result["rejected"].as_array().unwrap().len(), 1);
    assert!(feeds
        .iter()
        .all(|f| f["title"].as_str().unwrap().chars().count() <= 120
            && f["category"].as_str().unwrap().chars().count() <= 80));
}
