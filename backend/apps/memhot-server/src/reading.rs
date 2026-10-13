//! Tenant-authorized reading history, bookmarks, and conservatively linked events.
use crate::{
    auth::User,
    error::{Error, Result},
    report_access, research, App,
};
use axum::{
    extract::{Path, Query, State},
    Json,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rusqlite::params;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashSet};

#[path = "reading_materials.rs"]
mod materials;
#[path = "reading_translation.rs"]
mod translation;
pub use materials::material;
pub use translation::{queue_translation, translate};

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadingQuery {
    pub period: Option<String>,
    pub q: Option<String>,
    pub saved: Option<bool>,
    pub category: Option<String>,
    pub kind: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<usize>,
}
pub(super) fn stories(report: &Value) -> impl Iterator<Item = &Value> {
    report["editorial"]["stories"]
        .as_array()
        .into_iter()
        .flatten()
        .chain(
            report["editorial"]["flashes"]
                .as_array()
                .into_iter()
                .flatten(),
        )
}
fn date(report: &Value) -> &str {
    ["availableAt", "createdAt"]
        .into_iter()
        .find_map(|k| report[k].as_str())
        .unwrap_or("")
}
fn normalized_date(date: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(date)
        .map(|at| {
            at.with_timezone(&chrono::Utc)
                .to_rfc3339_opts(chrono::SecondsFormat::Nanos, true)
        })
        .unwrap_or_default()
}
fn key(report: &Value) -> (String, &str) {
    (
        normalized_date(date(report)),
        report["id"].as_str().unwrap_or(""),
    )
}
fn contains(report: &Value, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    ["title", "summary", "body", "question"].iter().any(|k| {
        report[k]
            .as_str()
            .is_some_and(|s| s.to_lowercase().contains(query))
    }) || stories(report).any(|s| {
        ["title", "summary", "whyItMatters", "readingGuide"]
            .iter()
            .any(|k| {
                s[k].as_str()
                    .is_some_and(|s| s.to_lowercase().contains(query))
            })
    }) || report["sources"].as_array().into_iter().flatten().any(|s| {
        ["title", "excerpt"].iter().any(|k| {
            s[k].as_str()
                .is_some_and(|s| s.to_lowercase().contains(query))
        })
    })
}
fn owned_ids(
    app: &App,
    user: &str,
    kind: Option<&str>,
    since: Option<&str>,
    saved: bool,
) -> Result<Vec<String>> {
    // Keep only tiny owned IDs in memory. Full-text queries scan one hydrated report at a time.
    let conn = app.store.conn()?;
    let mut stmt = conn.prepare("SELECT r.id,COALESCE(json_extract(r.data,'$.availableAt'),json_extract(r.data,'$.createdAt')) FROM records r WHERE r.user_id=?1 AND r.kind='report'
        AND (julianday(json_extract(r.data,'$.availableAt')) IS NULL OR julianday(json_extract(r.data,'$.availableAt'))<=julianday('now'))
        AND (?2 IS NULL OR json_extract(r.data,'$.kind')=?2)
        AND (?3 IS NULL OR julianday(COALESCE(json_extract(r.data,'$.availableAt'),json_extract(r.data,'$.createdAt')))>=julianday(?3))
        AND (?4=0 OR EXISTS(SELECT 1 FROM records s WHERE s.user_id=r.user_id AND s.kind='report_saved' AND s.id=r.id AND json_extract(s.data,'$.saved')=1))
        ORDER BY r.created DESC,r.id DESC")?;
    let rows = stmt.query_map(params![user, kind, since, saved], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
    })?;
    let mut ids = rows
        .map(|row| {
            let (id, date) = row?;
            Ok((id, normalized_date(date.as_deref().unwrap_or_default())))
        })
        .collect::<Result<Vec<_>>>()?;
    // SQLite julianday rounds sub-millisecond timestamps. Match the opaque cursor exactly.
    ids.sort_by(|a, b| (&b.1, &b.0).cmp(&(&a.1, &a.0)));
    Ok(ids.into_iter().map(|(id, _)| id).collect())
}
pub fn incomplete(report: &Value) -> bool {
    report["status"] == "incomplete"
        || (report["kind"] == "digest"
            && stories(report).next().is_none()
            && report["editorial"]["quietIssue"] != true)
        || (report["kind"] == "research" && report["claims"].as_array().is_some_and(Vec::is_empty))
}
fn summary(mut report: Value, saved: bool) -> Value {
    report["saved"] = json!(saved);
    report["sourceCount"] = json!(report["sources"].as_array().map_or(0, Vec::len));
    if incomplete(&report) {
        report["status"] = json!("incomplete");
    }
    report["body"] = json!("");
    report["sources"] = json!([]);
    if let Some(map) = report.as_object_mut() {
        map.remove("claims");
    }
    report
}
pub async fn list(
    State(app): State<App>,
    User(user): User,
    Query(input): Query<ReadingQuery>,
) -> Result<Json<Value>> {
    let period = input.period.as_deref().unwrap_or("all");
    if !matches!(period, "all" | "week" | "month")
        || input
            .kind
            .as_deref()
            .is_some_and(|k| !matches!(k, "research" | "digest"))
    {
        return Err(Error::bad("阅读时间范围或报告类型无效"));
    }
    let limit = input.limit.unwrap_or(20);
    let q = input.q.as_deref().unwrap_or("").trim().to_lowercase();
    let category = input.category.as_deref().unwrap_or("").trim();
    if !(1..=50).contains(&limit) || q.chars().count() > 200 || category.chars().count() > 80 {
        return Err(Error::bad(
            "每页须为1至50份，搜索词不超过200字，分类不超过80字",
        ));
    }
    let fingerprint = crate::hash(
        json!([
            user,
            period,
            q,
            input.saved.unwrap_or(false),
            category,
            input.kind
        ])
        .to_string()
        .as_bytes(),
    );
    let boundary = input
        .cursor
        .as_deref()
        .map(|cursor| -> Result<(String, String)> {
            if cursor.len() > 1024 {
                return Err(Error::bad("阅读游标无效，请从第一页重试"));
            }
            let bytes = URL_SAFE_NO_PAD
                .decode(cursor)
                .map_err(|_| Error::bad("阅读游标无效，请从第一页重试"))?;
            let value: Value = serde_json::from_slice(&bytes)
                .map_err(|_| Error::bad("阅读游标无效，请从第一页重试"))?;
            if value["filter"] != fingerprint {
                return Err(Error::bad("筛选条件已改变，请从第一页重试"));
            }
            Ok((
                value["date"]
                    .as_str()
                    .ok_or_else(|| Error::bad("阅读游标无效"))?
                    .into(),
                value["id"]
                    .as_str()
                    .ok_or_else(|| Error::bad("阅读游标无效"))?
                    .into(),
            ))
        })
        .transpose()?;
    let saved_ids: HashSet<String> = app
        .store
        .list_all(&user, "report_saved")?
        .into_iter()
        .filter(|v| v["saved"] == true)
        .filter_map(|v| v["id"].as_str().map(str::to_owned))
        .collect();
    let since = match period {
        "week" => Some(chrono::Utc::now() - chrono::Duration::days(7)),
        "month" => Some(chrono::Utc::now() - chrono::Duration::days(30)),
        _ => None,
    }
    .map(|at| at.to_rfc3339());
    let mut reports = Vec::new();
    let mut categories = BTreeSet::new();
    let (mut count, mut story_count, mut incomplete_count) = (0, 0, 0);
    let mut heads = Vec::new();
    for id in owned_ids(
        &app,
        &user,
        input.kind.as_deref(),
        since.as_deref(),
        input.saved.unwrap_or(false),
    )? {
        let report = match report_access::get(&app, &user, &id) {
            Ok(report) => report,
            Err(e) if e.code == "not_found" => continue,
            Err(e) => return Err(e),
        };
        if !contains(&report, &q) {
            continue;
        }
        categories.extend(
            stories(&report)
                .filter_map(|s| s["category"].as_str())
                .filter(|s| !s.is_empty())
                .map(str::to_owned),
        );
        if !category.is_empty() && !stories(&report).any(|s| s["category"] == category) {
            continue;
        }
        if incomplete(&report) {
            incomplete_count += 1;
        } else {
            count += 1;
            story_count += stories(&report).count();
        }
        if !incomplete(&report) && heads.len() < 3 {
            heads.push(
                stories(&report)
                    .find_map(|s| s["title"].as_str())
                    .or_else(|| report["title"].as_str())
                    .unwrap_or("未命名报告")
                    .to_owned(),
            );
        }
        if reports.len() <= limit
            && boundary.as_ref().is_none_or(|(date, id)| {
                let key = key(&report);
                (key.0.as_str(), key.1) < (date.as_str(), id.as_str())
            })
        {
            let saved = saved_ids.contains(&id);
            reports.push(summary(report, saved));
        }
    }
    let heads = heads.join("；");
    let title = match period {
        "week" => "近7天阅读回顾",
        "month" => "近30天阅读回顾",
        _ => "阅读历史",
    };
    let mut overview_summary = if count == 0 {
        "当前筛选下还没有已交付报告。".to_owned()
    } else {
        format!("已交付{count}份报告，包含{story_count}条事件。近期标题：{heads}")
    };
    if incomplete_count > 0 {
        overview_summary.push_str(&format!("另保留{incomplete_count}份未完成历史记录。"));
    }
    let overview = json!({"title":title,"summary":overview_summary,"reportCount":count,"storyCount":story_count});
    let next = (reports.len() > limit).then(|| {
        let last = &reports[limit - 1];
        URL_SAFE_NO_PAD
            .encode(json!({"date":key(last).0,"id":last["id"],"filter":fingerprint}).to_string())
    });
    reports.truncate(limit);
    let mut result = json!({"reports":reports,"overview":overview,"categories":categories});
    if let Some(next) = next {
        result["nextCursor"] = json!(next);
    }
    Ok(Json(result))
}

/// Detail endpoints must not expose a stale bookmark after its report is deleted.
pub fn saved(app: &App, user: &str, id: &str) -> Result<bool> {
    report_access::get(app, user, id)?;
    Ok(app.store.conn()?.query_row("SELECT EXISTS(SELECT 1 FROM records WHERE user_id=? AND kind='report_saved' AND id=? AND json_extract(data,'$.saved')=1)", params![user,id], |r| r.get(0))?)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Saved {
    pub saved: bool,
}
pub async fn set_saved(
    State(app): State<App>,
    User(user): User,
    Path(id): Path<String>,
    Json(input): Json<Saved>,
) -> Result<Json<Value>> {
    let mut conn = app.store.conn()?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let report = crate::persistence::current(&tx, &user, "report", &id)?
        .filter(research::available)
        .ok_or_else(Error::missing)?;
    if input.saved {
        let old = crate::persistence::current(&tx, &user, "report_saved", &id)?;
        crate::persistence::put(
            &tx,
            &user,
            "report_saved",
            &json!({"id":report["id"],"saved":true,"createdAt":old.map(|v| v["createdAt"].clone()).unwrap_or_else(||json!(crate::now()))}),
        )?;
    } else {
        tx.execute(
            "DELETE FROM records WHERE user_id=? AND kind='report_saved' AND id=?",
            params![user, id],
        )?;
    }
    tx.commit()?;
    Ok(Json(json!({"id":id,"saved":input.saved})))
}
fn urls(report: &Value, story: &Value) -> HashSet<String> {
    let ids = story["evidenceIds"].as_array().cloned().unwrap_or_default();
    report["sources"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|s| ids.contains(&s["id"]))
        .filter_map(|s| s["url"].as_str())
        .filter_map(|url| crate::network::validate_url(url).ok())
        .map(|mut url| {
            url.set_fragment(None);
            url.to_string()
        })
        .collect()
}
fn same_topic(a: &Value, b: &Value) -> bool {
    for field in ["catalogTopicId", "thoughtId", "question"] {
        if let (Some(a), Some(b)) = (
            a[field].as_str().filter(|s| !s.is_empty()),
            b[field].as_str().filter(|s| !s.is_empty()),
        ) {
            return a == b;
        }
    }
    true
}
pub async fn event(
    State(app): State<App>,
    User(user): User,
    Path((id, story_id)): Path<(String, String)>,
) -> Result<Json<Value>> {
    let report = report_access::get(&app, &user, &id)?;
    let story = stories(&report)
        .find(|s| s["id"] == story_id)
        .ok_or_else(Error::missing)?;
    let source_urls = urls(&report, story);
    let mut related = Vec::new();
    for candidate_id in owned_ids(&app, &user, None, None, false)?
        .into_iter()
        .filter(|candidate| candidate != &id)
    {
        let candidate = match report_access::get(&app, &user, &candidate_id) {
            Ok(report) => report,
            Err(e) if e.code == "not_found" => continue,
            Err(e) => return Err(e),
        };
        for item in stories(&candidate) {
            let same_key = story["eventKey"]
                .as_str()
                .filter(|s| !s.is_empty())
                .is_some_and(|key| item["eventKey"] == key);
            let shared_url = !source_urls.is_disjoint(&urls(&candidate, item));
            let same_category = story["category"]
                .as_str()
                .filter(|s| !s.is_empty())
                .is_some_and(|category| item["category"] == category);
            if same_key || shared_url || (same_category && same_topic(&report, &candidate)) {
                if item["id"].as_str().is_none() {
                    continue;
                }
                related.push(json!({"reportId":candidate["id"],"storyId":item["id"],"title":item["title"],"summary":item["summary"],"date":date(&candidate),"relationship":if same_key || shared_url {"same_event"} else {"related"}}));
            }
            if related.len() == 20 {
                break;
            }
        }
        if related.len() == 20 {
            break;
        }
    }
    Ok(Json(json!({"story":story,"related":related})))
}
