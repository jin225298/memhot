//! Licensed, opt-in AIHOT discovery. Aggregated text is never original-page evidence.
use crate::{
    agent::Scope,
    discovery::Material,
    error::{Error, Result},
};
use serde_json::{json, Value};

use crate::mcp_transport::{self as transport, Request, Server};
pub const ENDPOINT: &str = "https://aihot.news/api/mcp";
pub const WARNING: &str = "AIHOT 聚合摘要仅作发现；读取原始链接后才能引用事实，不代表已读原文。";
pub fn enabled() -> bool {
    std::env::var("MEMHOT_AIHOT_ENABLED").as_deref() == Ok("1")
}

pub struct Discovery {
    pub materials: Vec<Material>,
    pub stories: Value,
    pub attribution: Value,
    pub overview: Option<Value>,
}
pub fn public_request(catalog: &str, action: &str) -> Result<Request> {
    if !matches!(catalog, "ai" | "robotics") {
        return Err(Error::bad("AIHOT 公共请求仅支持 AI 与机器人目录"));
    }
    if catalog == "robotics" && action != "search" {
        return Err(Error::bad("机器人目录只允许固定相关主题查询"));
    }
    let query = if catalog == "robotics" {
        "robotics embodied intelligence"
    } else {
        "artificial intelligence"
    };
    match action {
        "latest" => Ok(Request {
            tool: "aihot_get_latest",
            args: json!({"window":"24h","mode":"selected","limit":10}),
        }),
        "search" => Ok(Request {
            tool: "aihot_search",
            args: json!({"q":query,"window":"7d","limit":10}),
        }),
        "hot" => Ok(Request {
            tool: "aihot_get_hot_topics",
            args: json!({"limit":10}),
        }),
        "daily" => Ok(Request {
            tool: "aihot_get_daily",
            args: json!({}),
        }),
        "weekly" => Ok(Request {
            tool: "aihot_get_weekly",
            args: json!({}),
        }),
        "monthly" => Ok(Request {
            tool: "aihot_get_monthly",
            args: json!({}),
        }),
        _ => Err(Error::bad("AIHOT 公共操作无效")),
    }
}
pub fn request(scope: &Scope, args: &Value, stories: &Value) -> Result<Request> {
    let fields = args
        .as_object()
        .ok_or_else(|| Error::bad("AIHOT 参数无效"))?;
    if fields
        .keys()
        .any(|key| !["action", "query", "period", "storyId"].contains(&key.as_str()))
    {
        return Err(Error::bad("AIHOT 包含未知参数"));
    }
    let action = args["action"]
        .as_str()
        .ok_or_else(|| Error::bad("AIHOT 操作无效"))?;
    if fields.keys().any(|key| {
        key != "action"
            && match action {
                "search" => key != "query",
                "story" => key != "storyId",
                "daily" | "weekly" | "monthly" => key != "period",
                _ => true,
            }
    }) {
        return Err(Error::bad("AIHOT 参数与操作不匹配"));
    }
    if let Scope::Public { catalog_id, .. } = scope {
        if fields.contains_key("query") || fields.contains_key("period") {
            return Err(Error::bad("公共 AIHOT 不接受私人查询或日期"));
        }
        if action != "story" {
            return public_request(catalog_id, action);
        }
        if catalog_id != "ai" {
            return Err(Error::bad("AIHOT 公共事件仅支持 AI 目录"));
        }
    }
    if action == "story" {
        let id = args["storyId"]
            .as_str()
            .filter(|id| {
                !id.is_empty()
                    && id.len() <= 128
                    && id
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
            })
            .ok_or_else(|| Error::bad("事件 ID 无效"))?;
        if !stories
            .as_array()
            .is_some_and(|items| items.iter().any(|item| item["id"] == id))
        {
            return Err(Error::bad("事件 ID 必须来自本轮 AIHOT 热点返回链接"));
        }
        return Ok(Request {
            tool: "aihot_get_story",
            args: json!({"public_id":id,"report_limit":10}),
        });
    }
    if fields.contains_key("storyId") {
        return Err(Error::bad("此 AIHOT 操作不接受事件 ID"));
    }
    if action == "search" {
        let raw = args["query"].as_str().unwrap_or_default();
        let query = crate::research::query_safe(raw);
        if !(2..=180).contains(&query.chars().count())
            || raw.len() > 720
            || raw.contains('@')
            || raw.contains("://")
            || raw.contains(['\n', '\r'])
        {
            return Err(Error::bad(
                "AIHOT 查询须为不含联系方式、网址或私人原文的公开主题",
            ));
        }
        return Ok(Request {
            tool: "aihot_search",
            args: json!({"q":query,"window":"7d","limit":10}),
        });
    }
    if fields.contains_key("query") {
        return Err(Error::bad("此 AIHOT 操作不接受查询词"));
    }
    let mut request = public_request("ai", action)?;
    if let Some(period) = args.get("period") {
        let period = period
            .as_str()
            .ok_or_else(|| Error::bad("AIHOT 日期无效"))?;
        let valid = match action {
            "daily" => {
                chrono::NaiveDate::parse_from_str(period, "%Y-%m-%d").is_ok() && period.len() == 10
            }
            "weekly" => {
                chrono::NaiveDate::parse_from_str(&format!("{period}-1"), "%G-W%V-%u").is_ok()
                    && period.len() == 8
            }
            "monthly" => {
                chrono::NaiveDate::parse_from_str(&format!("{period}-01"), "%Y-%m-%d").is_ok()
                    && period.len() == 7
            }
            _ => false,
        };
        if !valid {
            return Err(Error::bad("AIHOT 日期或操作不匹配"));
        }
        request.args[match action {
            "daily" => "date",
            "weekly" => "week",
            _ => "month",
        }] = json!(period);
    }
    Ok(request)
}
pub fn decode(value: &Value) -> Result<Discovery> {
    let sources = value["sources"]
        .as_array()
        .filter(|v| v.len() <= 20)
        .ok_or_else(|| Error::upstream("aihot_response", "AIHOT 来源格式无效"))?;
    let mut materials = Vec::new();
    for source in sources {
        let url = crate::network::validate_url(source["url"].as_str().unwrap_or_default())?;
        let via = crate::network::validate_url(
            source["attribution"]["url"].as_str().unwrap_or_default(),
        )?;
        if via.scheme() != "https"
            || via.host_str() != Some("aihot.news")
            || via.port().is_some()
            || via.query().is_some()
            || via.fragment().is_some()
            || source["fulltext"] != false
            || source["attribution"]["name"] != "AIHOT"
        {
            return Err(Error::upstream("aihot_response", "AIHOT 来源归属无效"));
        }
        let title = source["title"]
            .as_str()
            .filter(|v| !v.trim().is_empty() && v.len() <= 1000)
            .ok_or_else(|| Error::upstream("aihot_response", "AIHOT 标题无效"))?;
        let text = source["text"]
            .as_str()
            .filter(|v| v.len() <= 6000)
            .ok_or_else(|| Error::upstream("aihot_response", "AIHOT 摘要无效"))?;
        materials.push(Material {
            title: title.into(),
            url: url.to_string(),
            text: text.into(),
            published: None,
            discovered: Some(crate::now()),
            discovered_via: Some(via.to_string()),
            publication_source: "search_reported_publication",
            publisher: url.host_str().unwrap_or_default().into(),
            fulltext: false,
            pdf: None,
        });
    }
    let stories = value["stories"]
        .as_array()
        .filter(|v| v.len() <= 10)
        .ok_or_else(|| Error::upstream("aihot_response", "AIHOT 事件列表无效"))?;
    for story in stories {
        let id = story["id"]
            .as_str()
            .filter(|id| {
                !id.is_empty()
                    && id.len() <= 128
                    && id
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
            })
            .ok_or_else(|| Error::bad("AIHOT 事件 ID 无效"))?;
        let url = crate::network::validate_url(story["url"].as_str().unwrap_or_default())?;
        if url.scheme() != "https"
            || url.host_str() != Some("aihot.news")
            || url.port().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || url.path().rsplit('/').next() != Some(id)
        {
            return Err(Error::bad("AIHOT 事件链接无效"));
        }
    }
    let overview = value.get("overview").cloned();
    if let Some(context) = &overview {
        let url = crate::network::validate_url(
            context["attribution"]["url"].as_str().unwrap_or_default(),
        )?;
        if !context.is_object()
            || context.to_string().len() > 16_384
            || url.scheme() != "https"
            || url.host_str() != Some("aihot.news")
            || url.port().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || context["attribution"]["name"] != "AIHOT"
        {
            return Err(Error::bad("AIHOT 编辑背景或归属无效"));
        }
    }
    Ok(Discovery {
        materials,
        stories: json!(stories),
        attribution: json!({"name":"AIHOT","url":"https://aihot.news/"}),
        overview,
    })
}
pub async fn fetch(request: &Request) -> Result<Value> {
    if !enabled() {
        return Err(Error::upstream("aihot_disabled", "AIHOT 接入未启用"));
    }
    let value = transport::run(request, &Server::aihot()).await?;
    decode(&value)?;
    Ok(value)
}

#[cfg(test)]
#[path = "aihot_tests.rs"]
mod tests;
