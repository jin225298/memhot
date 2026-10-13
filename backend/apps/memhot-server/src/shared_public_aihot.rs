//! Licensed finished editions are shared reading, never independently verified source evidence.
use super::{acquire, commit, result, EditionResult, Reservation};
use crate::{
    error::{Error, Result},
    hash, App,
};
use chrono::{DateTime, Duration, NaiveDate, Utc};
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashSet;

const VERSION: &str = "aihot-syndicated-daily-v1";
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Daily {
    date: String,
    generated_at: String,
    window_start: String,
    window_end: String,
    attribution: Attribution,
    lead: Option<Lead>,
    sections: Vec<Section>,
    flashes: Vec<Item>,
}
#[derive(Deserialize)]
struct Attribution {
    name: String,
    url: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Lead {
    title: String,
    lead_paragraph: String,
}
#[derive(Deserialize)]
struct Section {
    label: String,
    items: Vec<Item>,
}
#[derive(Deserialize)]
struct Item {
    title: String,
    #[serde(default)]
    summary: String,
    source: Publisher,
    links: Links,
    attribution: Attribution,
}
#[derive(Deserialize)]
struct Publisher {
    name: String,
}
#[derive(Deserialize)]
struct Links {
    original: String,
}
fn invalid() -> Error {
    Error::upstream("aihot_response", "AIHOT 日报格式或时间无效，未导入")
}
fn text(value: &str, max: usize, empty: bool) -> Result<()> {
    if value.chars().count() > max || (!empty && value.trim().is_empty()) {
        return Err(invalid());
    }
    Ok(())
}
fn attribution(value: &Attribution) -> Result<()> {
    let url = crate::network::validate_url(&value.url)?;
    if value.name != "AIHOT"
        || url.scheme() != "https"
        || url.host_str() != Some("aihot.news")
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid());
    }
    Ok(())
}
fn timestamp(value: &str) -> Result<DateTime<Utc>> {
    crate::freshness::publication(value).ok_or_else(invalid)
}
fn previous(app: &App) -> Result<Option<Value>> {
    let raw: Option<String> = app.store.conn()?.query_row(
        "SELECT body FROM public_cache WHERE kind='edition' AND body IS NOT NULL AND retain_until>? AND json_extract(body,'$.pipelineVersion')=? ORDER BY json_extract(body,'$.issueDate') DESC LIMIT 1",
        params![Utc::now().timestamp(), VERSION], |r| r.get(0)).optional()?;
    raw.map(|s| serde_json::from_str(&s).map_err(Into::into))
        .transpose()
}
fn source(item: &Item, fetched: &str) -> Result<Value> {
    text(&item.title, 500, false)?;
    text(&item.summary, 6000, true)?;
    text(&item.source.name, 200, false)?;
    attribution(&item.attribution)?;
    if item.links.original.len() > 2048 {
        return Err(invalid());
    }
    let url = crate::network::validate_url(&item.links.original)?;
    // The API does not give publication dates for main entries. Even a flash's
    // fallback timestamp may be the issue time: do not invent an original date.
    Ok(
        json!({"id":hash(url.as_str().as_bytes())[..24],"title":item.title,"url":url.as_str(),
        "publisher":item.source.name,"publishedAt":null,"retrievedAt":fetched,
        "excerpt":item.summary,"stance":"context","retrievalKind":"search_snippet",
        "publicationDateSource":"unknown","attribution":{"name":"AIHOT","url":item.attribution.url},
        "contentOrigin":"external_editorial","citations":[]}),
    )
}
fn decode(raw: &Value, at: DateTime<Utc>) -> Result<Value> {
    if raw.to_string().len() > 64 * 1024 {
        return Err(invalid());
    }
    let daily: Daily = serde_json::from_value(raw.clone()).map_err(|_| invalid())?;
    attribution(&daily.attribution)?;
    let date = NaiveDate::parse_from_str(&daily.date, "%Y-%m-%d").map_err(|_| invalid())?;
    let start = timestamp(&daily.window_start)?;
    let end = timestamp(&daily.window_end)?;
    let generated = timestamp(&daily.generated_at)?;
    if daily.date.len() != 10
        || date != end.with_timezone(&chrono_tz::Asia::Shanghai).date_naive()
        || start >= end
        || end - start > Duration::hours(48)
        || end > at + Duration::minutes(10)
        || generated < end
        || generated > at + Duration::minutes(10)
        || daily.sections.len() > 12
        || daily.flashes.len() > 30
    {
        return Err(invalid());
    }
    let count = daily.sections.iter().map(|s| s.items.len()).sum::<usize>() + daily.flashes.len();
    if count > 60 || (count == 0 && daily.lead.is_none()) {
        return Err(invalid());
    }
    let fetched = at.to_rfc3339();
    let mut sources = Vec::new();
    let mut stories = Vec::new();
    let mut flashes = Vec::new();
    let mut sections = Vec::new();
    let mut urls = HashSet::new();
    let mut add = |item: &Item, category: &str| -> Result<Value> {
        let evidence = source(item, &fetched)?;
        // A malformed repeated entry must not become two events or silently lose text.
        if !urls.insert(evidence["url"].as_str().unwrap().to_owned()) {
            return Err(invalid());
        }
        let id = format!("aihot-story-{}", evidence["id"].as_str().unwrap());
        let story = json!({"id":id,"eventKey":id,"title":item.title,"summary":item.summary,
            "category":category,"evidenceIds":[evidence["id"]],"coverageCount":1,
            "contentOrigin":"external_editorial","tags":[]});
        sources.push(evidence);
        Ok(story)
    };
    for (i, section) in daily.sections.iter().enumerate() {
        text(&section.label, 100, false)?;
        let mut ids = Vec::new();
        for item in &section.items {
            let story = add(item, &section.label)?;
            ids.push(story["id"].clone());
            stories.push(story);
        }
        if !ids.is_empty() {
            sections.push(
                json!({"id":format!("aihot-section-{i}"),"title":section.label,"storyIds":ids}),
            );
        }
    }
    for item in &daily.flashes {
        flashes.push(add(item, "快讯")?);
    }
    let summary = if let Some(lead) = &daily.lead {
        text(&lead.title, 500, false)?;
        text(&lead.lead_paragraph, 6000, true)?;
        lead.lead_paragraph.clone()
    } else {
        String::new()
    };
    let lead = daily
        .lead
        .as_ref()
        .and_then(|lead| stories.iter().find(|s| s["title"] == lead.title))
        .or_else(|| stories.first())
        .map(|s| s["id"].clone())
        .unwrap_or(Value::Null);
    // The MCP has no explicit highlights, so do not fabricate a ranked top three.
    let mut body = format!(
        "来源：[AIHOT 日报 · {}]({})\n\n",
        daily.date, daily.attribution.url
    );
    for story in stories.iter().chain(&flashes) {
        body.push_str(&format!(
            "### {}\n\n{}\n\n",
            story["title"].as_str().unwrap(),
            story["summary"].as_str().unwrap()
        ));
    }
    let expected = (at.with_timezone(&chrono_tz::Asia::Shanghai) - Duration::hours(8)).date_naive();
    let stale = date < expected;
    let note = format!("内容由 AIHOT 编排，刊期 {}。保留其栏目、摘要与原文链接；Memhot 未将聚合摘要当作已核验原文。", daily.date);
    let publishers: HashSet<_> = sources
        .iter()
        .filter_map(|s| s["publisher"].as_str())
        .collect();
    let gaps = if stale {
        vec![format!(
            "AIHOT 尚未提供本期日报，当前展示 {} 原刊。",
            daily.date
        )]
    } else {
        vec![]
    };
    Ok(
        json!({"id":format!("aihot-daily-{}",daily.date),"title":format!("AIHOT 日报 · {}",daily.date),
        "kind":"digest","summary":summary,"body":body,"sources":sources,"gaps":gaps,
        "status":if stale {"partial"} else {"ready"},"catalogTopicId":"ai","language":"zh-CN",
        "pipelineVersion":VERSION,"issueDate":daily.date,"utcDate":end.date_naive().to_string(),
        "createdAt":daily.generated_at,"editionWindowStart":daily.window_start,"editionWindowEnd":daily.window_end,
        "contentOrigin":"external_editorial","attribution":{"name":"AIHOT","url":daily.attribution.url},
        "editorial":{"version":VERSION,"stories":stories,"flashes":flashes,"sections":sections,"highlights":[],
            "leadStoryId":lead,"overviewStatus":"ready","overviewMode":"upstream_editorial","citationReview":"external_editorial",
            "quietIssue":count==0,"coverageNote":note,"acceptedStories":count,"agentSteps":0,
            "metrics":{"storyCount":count,"sourceCount":sources.len(),"publisherCount":publishers.len(),"readingMinutes":(body.chars().count()+499)/500}}}),
    )
}
pub(super) async fn edition(app: &App) -> Result<EditionResult> {
    let fetched = super::aihot_cached(app, "ai", "daily").await;
    let report = fetched.and_then(|value| decode(&value["dailyReport"], Utc::now()));
    match report {
        Ok(value) => {
            let key = value["id"].as_str().ok_or_else(invalid)?;
            let mut value = match acquire(app, key, "edition")? {
                Reservation::Refresh(lease) => {
                    // Recheck revisions after five minutes. The stored issue's reading
                    // freshness lasts until the next daily cutoff, not five minutes.
                    if !commit(app, &lease, &value, 300, 30 * 86400)? {
                        return Ok(result(
                            previous(app)?,
                            "stale",
                            Some("AIHOT 日报同步中，请稍后刷新"),
                        ));
                    }
                    value
                }
                // Return the committed snapshot: account references must hydrate
                // exactly the edition returned by this request, even during refresh.
                Reservation::Ready(stored) => stored,
                Reservation::Waiting(stored) => {
                    return Ok(result(
                        stored,
                        "stale",
                        Some("AIHOT 日报同步中，请稍后刷新"),
                    ));
                }
            };
            let stale = is_stale(&value, Utc::now());
            if stale {
                value["status"] = json!("partial");
            }
            Ok(result(
                Some(value),
                if stale { "stale" } else { "ready" },
                None,
            ))
        }
        Err(error) => {
            eprintln!("aihot_daily code={}", error.code);
            let old = previous(app)?;
            Ok(result(
                old.clone(),
                if old.is_some() {
                    "stale"
                } else {
                    "unavailable"
                },
                Some("AIHOT 日报暂时无法同步，已有刊物保留原日期，请稍后刷新"),
            ))
        }
    }
}

pub(super) fn current(value: &Value, at: DateTime<Utc>) -> bool {
    value["pipelineVersion"] == VERSION
        && value["issueDate"]
            .as_str()
            .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
            .is_some_and(|d| {
                d >= (at.with_timezone(&chrono_tz::Asia::Shanghai) - Duration::hours(8))
                    .date_naive()
            })
}
pub(super) fn is_stale(value: &Value, at: DateTime<Utc>) -> bool {
    value["pipelineVersion"] == VERSION && !current(value, at)
}

#[cfg(test)]
#[path = "shared_public_aihot_tests.rs"]
mod tests;
