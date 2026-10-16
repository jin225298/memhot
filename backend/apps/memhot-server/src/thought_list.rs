//! Search and archive filters apply to the whole tenant collection before pagination.
use crate::{
    auth::User,
    error::{Error, Result},
    App,
};
use axum::{
    extract::{Query, State},
    Json,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::{Duration, NaiveDate, TimeZone, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeSet;

#[path = "thought_scoped.rs"]
mod scoped;
#[derive(Deserialize)]
pub struct Page {
    scope: Option<String>,
    before: Option<String>,
    limit: Option<u32>,
    q: Option<String>,
    status: Option<String>,
    period: Option<String>,
    category: Option<String>,
    date: Option<String>,
}
#[derive(Serialize)]
struct Filters {
    q: Option<String>,
    status: String,
    category: Option<String>,
    start: Option<String>,
    end: Option<String>,
    timezone: String,
}
#[derive(Deserialize, Serialize)]
struct BoundCursor {
    created: String,
    id: String,
    filter: String,
}
#[derive(Deserialize)]
#[serde(untagged)]
enum Cursor {
    Bound(BoundCursor),
    Legacy((String, String)),
}
const FILTER: &str = r"FROM records WHERE user_id=?1 AND kind='thought'
 AND COALESCE(json_extract(data,'$.status'),'open')!='deleted'
 AND (?2 IS NULL OR COALESCE(json_extract(data,'$.title'),'') LIKE ?2 ESCAPE '\'
 OR COALESCE(json_extract(data,'$.question'),'') LIKE ?2 ESCAPE '\'
 OR COALESCE(json_extract(data,'$.summary'),'') LIKE ?2 ESCAPE '\'
 OR COALESCE(json_extract(data,'$.quote'),'') LIKE ?2 ESCAPE '\')
 AND (?3='all' OR (?3='archived' AND json_extract(data,'$.status') IN ('dismissed','resolved'))
 OR (?3='active' AND COALESCE(json_extract(data,'$.status'),'open') NOT IN ('dismissed','resolved')))
 AND (?4 IS NULL OR COALESCE(NULLIF(TRIM(json_extract(data,'$.category')),''),'未分类')=?4)
 AND (?5 IS NULL OR julianday(created)>=julianday(?5))
 AND (?6 IS NULL OR julianday(created)<julianday(?6))";

// Some timezones skip midnight or an entire date. Use its first representable instant.
fn midnight(date: NaiveDate, tz: chrono_tz::Tz) -> Result<String> {
    let base = date
        .and_hms_opt(0, 0, 0)
        .ok_or_else(|| Error::bad("日期无效"))?;
    (0..=48)
        .find_map(|hours| {
            tz.from_local_datetime(&(base + Duration::hours(hours)))
                .earliest()
        })
        .map(|at| at.with_timezone(&Utc).to_rfc3339())
        .ok_or_else(|| Error::bad("日期无效"))
}
fn filters(page: &Page, tz: chrono_tz::Tz) -> Result<Filters> {
    let status = page.status.as_deref().unwrap_or("all");
    if !matches!(status, "all" | "active" | "archived") {
        return Err(Error::bad("思考状态筛选无效"));
    }
    let period = page.period.as_deref().unwrap_or("all");
    if !matches!(period, "all" | "week" | "month") {
        return Err(Error::bad("思考时间筛选无效"));
    }
    if page.q.as_ref().is_some_and(|q| q.chars().count() > 200) {
        return Err(Error::bad("搜索内容不能超过200字"));
    }
    let q = page
        .q
        .as_ref()
        .map(|q| q.trim())
        .filter(|q| !q.is_empty())
        .map(|q| {
            format!(
                "%{}%",
                q.replace('\\', "\\\\")
                    .replace('%', "\\%")
                    .replace('_', "\\_")
            )
        });
    if page
        .category
        .as_ref()
        .is_some_and(|c| c.trim().chars().count() > 40)
    {
        return Err(Error::bad("分类不能超过40字"));
    }
    let category = page.category.as_ref().map(|c| {
        if c.trim().is_empty() {
            "未分类"
        } else {
            c.trim()
        }
        .to_owned()
    });
    let today = Utc::now().with_timezone(&tz).date_naive();
    let mut start = match period {
        "week" => Some(today - Duration::days(6)),
        "month" => Some(today - Duration::days(29)),
        _ => None,
    };
    let mut end = (period != "all").then_some(today + Duration::days(1));
    if let Some(raw) = &page.date {
        let date = NaiveDate::parse_from_str(raw, "%Y-%m-%d")
            .ok()
            .filter(|d| d.to_string() == *raw)
            .ok_or_else(|| Error::bad("日期须为YYYY-MM-DD"))?;
        start = Some(start.map_or(date, |s| s.max(date)));
        let next = date.succ_opt().ok_or_else(|| Error::bad("日期无效"))?;
        end = Some(end.map_or(next, |e| e.min(next)));
    }
    Ok(Filters {
        q,
        status: status.to_owned(),
        category,
        start: start.map(|d| midnight(d, tz)).transpose()?,
        end: end.map(|d| midnight(d, tz)).transpose()?,
        timezone: tz.name().to_owned(),
    })
}
fn cursor(raw: Option<&str>, filter: &str) -> Result<Option<(String, String)>> {
    let Some(raw) = raw else { return Ok(None) };
    if raw.len() > 2048 {
        return Err(Error::bad("分页位置无效"));
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(raw)
        .map_err(|_| Error::bad("分页位置无效"))?;
    let cursor: Cursor = serde_json::from_slice(&bytes).map_err(|_| Error::bad("分页位置无效"))?;
    let (created, id) = match cursor {
        Cursor::Bound(c) => {
            if c.filter != filter {
                return Err(Error::bad("筛选条件已改变，请从第一页开始"));
            }
            (c.created, c.id)
        }
        Cursor::Legacy(pair) => pair,
    };
    if created.len() > 64 || id.len() > 128 {
        return Err(Error::bad("分页位置无效"));
    }
    Ok(Some((created, id)))
}
fn facets(conn: &Connection, user: &str, tz: chrono_tz::Tz) -> Result<(Vec<String>, Vec<String>)> {
    let mut stmt = conn.prepare("SELECT DISTINCT COALESCE(NULLIF(TRIM(json_extract(data,'$.category')),''),'未分类'),created FROM records WHERE user_id=? AND kind='thought' AND COALESCE(json_extract(data,'$.status'),'open')!='deleted'")?;
    let rows = stmt.query_map([user], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })?;
    let mut categories = BTreeSet::new();
    let mut dates = BTreeSet::new();
    for row in rows {
        let (category, created) = row?;
        categories.insert(category);
        if let Ok(at) = chrono::DateTime::parse_from_rfc3339(&created) {
            dates.insert(at.with_timezone(&tz).date_naive().to_string());
            if dates.len() > 366 {
                dates.pop_first();
            }
        }
    }
    Ok((
        categories.into_iter().collect(),
        dates.into_iter().rev().collect(),
    ))
}
pub async fn list(
    State(app): State<App>,
    User(user): User,
    Query(page): Query<Page>,
) -> Result<Json<Value>> {
    match page.scope.as_deref() {
        Some("today" | "history") => return scoped::list(app, user, page),
        None | Some("all") => {}
        _ => return Err(Error::bad("思考范围无效")),
    }
    let limit = page.limit.unwrap_or(50).clamp(1, 100);
    let profile = app.store.profile(&user)?;
    let tz = profile["timezone"]
        .as_str()
        .unwrap_or("Asia/Shanghai")
        .parse::<chrono_tz::Tz>()
        .unwrap_or(chrono_tz::Asia::Shanghai);
    let filters = filters(&page, tz)?;
    let binding = crate::hash(&serde_json::to_vec(&filters)?);
    let cursor = cursor(page.before.as_deref(), &binding)?;
    let conn = app.store.conn()?;
    // Counts ignore the status selector so both archive tabs retain useful totals.
    let (active, archived): (u64, u64) = conn.query_row(&format!("SELECT COALESCE(SUM(COALESCE(json_extract(data,'$.status'),'open') NOT IN ('dismissed','resolved')),0),COALESCE(SUM(json_extract(data,'$.status') IN ('dismissed','resolved')),0) {FILTER}"),
        params![user,filters.q,"all",filters.category,filters.start,filters.end],|r|Ok((r.get(0)?,r.get(1)?)))?;
    let total = match filters.status.as_str() {
        "active" => active,
        "archived" => archived,
        _ => active + archived,
    };
    let mut stmt = conn.prepare(&format!("SELECT data,created,id {FILTER} AND (?7 IS NULL OR created<?7 OR (created=?7 AND id<?8)) ORDER BY created DESC,id DESC LIMIT ?9"))?;
    let rows = stmt.query_map(
        params![
            user,
            filters.q,
            filters.status,
            filters.category,
            filters.start,
            filters.end,
            cursor.as_ref().map(|c| &c.0),
            cursor.as_ref().map(|c| &c.1),
            limit + 1
        ],
        |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        },
    )?;
    let mut rows = rows.collect::<std::result::Result<Vec<_>, _>>()?;
    let more = rows.len() > limit as usize;
    rows.truncate(limit as usize);
    let next = if more {
        rows.last().map(|(_, created, id)| {
            URL_SAFE_NO_PAD.encode(
                serde_json::to_vec(&BoundCursor {
                    created: created.clone(),
                    id: id.clone(),
                    filter: binding,
                })
                .unwrap_or_default(),
            )
        })
    } else {
        None
    };
    let items = rows
        .into_iter()
        .map(|(raw, _, _)| {
            let mut value: Value = serde_json::from_str(&raw)?;
            if value["category"]
                .as_str()
                .is_none_or(|s| s.trim().is_empty())
            {
                value["category"] = json!("未分类");
            }
            Ok(value)
        })
        .collect::<Result<Vec<_>>>()?;
    let (categories, dates) = facets(&conn, &user, tz)?;
    Ok(Json(
        json!({"items":items,"nextCursor":next,"total":total,"categories":categories,"dates":dates,
        "counts":{"active":active,"archived":archived}}),
    ))
}
