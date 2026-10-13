//! Shared artifacts accept only server-owned catalog IDs, never a user's query/profile.
use crate::{
    error::{Error, Result},
    hash, id, network, App,
};
use chrono::{Duration, Timelike, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use serde_json::{json, Value};
use std::time::Duration as StdDuration;
pub const PIPELINE_VERSION: &str = "public-agent-editorial-v9-candidates";
#[path = "shared_public_catalog.rs"]
mod catalog_data;
#[path = "shared_public_history.rs"]
mod history;
#[path = "shared_public_inputs.rs"]
mod inputs;
#[path = "shared_public_retrieval.rs"]
mod retrieval;
#[path = "shared_public_aihot.rs"]
mod syndicated;
pub(crate) use history::seen as seen_sources;
pub(crate) fn first_party(material: &crate::discovery::Material) -> bool {
    material
        .discovered_via
        .as_deref()
        .and_then(catalog_data::policy)
        .is_some_and(|p| {
            p.official_abstract && catalog_data::publisher_matches(&material.url, p.domain)
        })
}
use catalog_data::CATALOG;
pub use catalog_data::{catalog, match_topics, CatalogTopic};
pub async fn agent_search(
    app: &App,
    catalog_id: &str,
    query: &str,
) -> Result<Vec<crate::discovery::Material>> {
    let topic = catalog()
        .iter()
        .find(|t| t.id == catalog_id)
        .ok_or_else(|| Error::bad("未知公共领域"))?;
    retrieval::agent_search(app, topic, query).await
}
/// Only fixed server catalog requests enter this cache. Private queries never call this path.
pub async fn aihot_cached(app: &App, catalog_id: &str, action: &str) -> Result<Value> {
    if !crate::aihot::enabled() {
        return Err(Error::upstream("aihot_disabled", "AIHOT 接入未启用"));
    }
    let request = crate::aihot::public_request(catalog_id, action)?;
    let key = format!("aihot-v2-{}", hash(request.start().to_string().as_bytes()));
    let lease = match acquire(app, &key, "aihot")? {
        Reservation::Ready(value) => return Ok(value),
        Reservation::Waiting(Some(value)) => return Ok(value),
        Reservation::Waiting(None) => {
            return Err(Error::upstream(
                "busy",
                "AIHOT 公共资料正在刷新或等待限流解除",
            ))
        }
        Reservation::Refresh(lease) => lease,
    };
    match crate::aihot::fetch(&request).await {
        Ok(value) => {
            if !commit(app, &lease, &value, 300, 3600)? {
                return Err(Error::upstream("busy", "AIHOT 公共缓存租约已变更"));
            }
            Ok(value)
        }
        Err(error) => {
            failed(app, &lease, error.code)?;
            Err(error)
        }
    }
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditionResult {
    pub edition: Option<Value>,
    pub cache_status: String,
    pub warning: Option<String>,
}
#[derive(Debug)]
pub struct CachedPage {
    pub url: String,
    pub text: String,
    pub content_type: String,
    pub cache_status: String,
    pub fetched_at: i64,
}
#[derive(Debug)]
struct Lease {
    key: String,
    token: String,
    previous: Option<Value>,
}
#[derive(Debug)]
enum Reservation {
    Ready(Value),
    Refresh(Lease),
    Waiting(Option<Value>),
}
pub fn migrate(conn: &Connection) -> Result<()> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS public_cache(cache_key TEXT PRIMARY KEY,kind TEXT NOT NULL,body TEXT,expires_at INTEGER NOT NULL DEFAULT 0,retain_until INTEGER NOT NULL DEFAULT 0,lease_token TEXT,lease_until INTEGER NOT NULL DEFAULT 0,retry_after INTEGER NOT NULL DEFAULT 0,last_error TEXT); CREATE TABLE IF NOT EXISTS public_urls(url TEXT PRIMARY KEY,registered_at INTEGER NOT NULL); CREATE INDEX IF NOT EXISTS public_cache_expiry ON public_cache(retain_until);")?;
    inputs::migrate(conn)?;
    for topic in CATALOG {
        for url in topic.feeds {
            conn.execute(
                "INSERT OR IGNORE INTO public_urls(url,registered_at) VALUES(?,?)",
                params![url, Utc::now().timestamp()],
            )?;
        }
    }
    Ok(())
}
fn acquire(app: &App, key: &str, kind: &str) -> Result<Reservation> {
    let mut conn = app.store.conn()?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let at = Utc::now().timestamp();
    tx.execute(
        "INSERT OR IGNORE INTO public_cache(cache_key,kind) VALUES(?,?)",
        params![key, kind],
    )?;
    let (raw,expires,lease,retry,retain):(Option<String>,i64,i64,i64,i64)=tx.query_row("SELECT body,expires_at,lease_until,retry_after,retain_until FROM public_cache WHERE cache_key=?",[key],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?)))?;
    let previous = if retain > at {
        raw.map(|s| serde_json::from_str(&s)).transpose()?
    } else {
        None
    };
    if expires > at {
        if let Some(value) = previous.clone() {
            return Ok(Reservation::Ready(value));
        }
    }
    if lease > at || retry > at {
        return Ok(Reservation::Waiting(previous));
    }
    let token = id();
    tx.execute(
        "UPDATE public_cache SET lease_token=?,lease_until=? WHERE cache_key=?",
        params![token, at + if kind == "edition" { 780 } else { 30 }, key],
    )?;
    tx.commit()?;
    Ok(Reservation::Refresh(Lease {
        key: key.into(),
        token,
        previous,
    }))
}
fn commit(app: &App, lease: &Lease, value: &Value, ttl: i64, retention: i64) -> Result<bool> {
    let at = Utc::now().timestamp();
    let changed=app.store.conn()?.execute("UPDATE public_cache SET body=?,expires_at=?,retain_until=?,lease_token=NULL,lease_until=0,retry_after=0,last_error=NULL WHERE cache_key=? AND lease_token=? AND lease_until>?",
        params![value.to_string(),at+ttl,at+retention,lease.key,lease.token,at])?;
    Ok(changed == 1)
}
fn failed(app: &App, lease: &Lease, code: &str) -> Result<()> {
    let at = Utc::now().timestamp();
    let terminal = matches!(
        code,
        "public_outcome_unknown"
            | "public_idempotency_conflict"
            | "public_retry_exhausted"
            | "public_inputs_expired"
            | "public_inputs_changed"
    ) || code.starts_with("editorial_");
    let retry_at = if code == "no_new_sources" {
        (edition_window(Utc::now()) + Duration::hours(6)).timestamp()
    } else if terminal {
        (at.div_euclid(86400) + 1) * 86400
    } else {
        at + 60
    };
    // Keep failed edition metadata through its retry window. Retention must not
    // erase a terminal paid-outcome fence merely because no report was published.
    app.store.conn()?.execute("UPDATE public_cache SET lease_token=NULL,lease_until=0,retry_after=?,last_error=?,retain_until=CASE WHEN kind='edition' AND body IS NULL THEN MAX(retain_until,?) ELSE retain_until END WHERE cache_key=? AND lease_token=?",params![retry_at,code,at+86400,lease.key,lease.token])?;
    Ok(())
}
fn safe_url(raw: &str) -> Result<String> {
    let url = network::validate_url(raw)?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(Error::bad(
            "共享缓存仅接受无凭据、查询参数或片段的公开HTTPS来源",
        ));
    }
    Ok(url.to_string())
}
fn register_public(app: &App, url: &str) -> Result<String> {
    let url = safe_url(url)?;
    app.store.conn()?.execute("INSERT INTO public_urls(url,registered_at) VALUES(?,?) ON CONFLICT(url) DO UPDATE SET registered_at=excluded.registered_at",params![url,Utc::now().timestamp()])?;
    Ok(url)
}
fn page(value: Value, status: &str) -> Result<CachedPage> {
    Ok(CachedPage {
        url: value["url"].as_str().ok_or_else(Error::internal)?.into(),
        text: value["text"].as_str().ok_or_else(Error::internal)?.into(),
        content_type: value["contentType"].as_str().unwrap_or_default().into(),
        cache_status: status.into(),
        fetched_at: value["fetchedAt"].as_i64().unwrap_or(0),
    })
}
/// Only URLs registered by fixed-catalog discovery are eligible. Arbitrary user URLs fail closed.
pub async fn cached_page(app: &App, url: &str) -> Result<CachedPage> {
    let url = safe_url(url)?;
    let authorized: bool = app.store.conn()?.query_row(
        "SELECT EXISTS(SELECT 1 FROM public_urls WHERE url=?)",
        [&url],
        |r| r.get(0),
    )?;
    if !authorized {
        return Err(Error::bad("此来源不属于固定公开目录，不能进入共享缓存"));
    }
    let key = format!("page-{}", hash(url.as_bytes()));
    let lease = match acquire(app, &key, "page")? {
        Reservation::Ready(value) => return page(value, "ready"),
        Reservation::Waiting(Some(value)) => return page(value, "stale"),
        Reservation::Waiting(None) => {
            return Err(Error::upstream("busy", "公共来源正在刷新，请稍后重试"))
        }
        Reservation::Refresh(lease) => lease,
    };
    let result=tokio::time::timeout(StdDuration::from_secs(15),network::fetch(&url)).await.map_err(|_|Error::upstream("timeout","公共来源获取超时")).and_then(|r|r).and_then(|fetched|{
        let final_url=safe_url(&fetched.url)?;
        Ok(json!({"url":final_url,"text":fetched.text,"contentType":fetched.content_type,"fetchedAt":Utc::now().timestamp()}))
    });
    match result {
        Ok(value) => {
            if !commit(app, &lease, &value, 1800, 86400)? {
                return Err(Error::upstream("busy", "公共来源刷新租约已变更"));
            }
            page(value, "ready")
        }
        Err(error) => {
            eprintln!("public_page={} code={}", lease.key, error.code);
            failed(app, &lease, error.code)?;
            if let Some(previous) = lease.previous {
                page(previous, "stale")
            } else {
                Err(error)
            }
        }
    }
}
fn edition_key(topic: &str, date: &str, language: &str) -> String {
    format!(
        "edition-{}",
        hash(format!("{topic}:{date}:{language}:{PIPELINE_VERSION}").as_bytes())
    )
}
fn edition_window(at: chrono::DateTime<Utc>) -> chrono::DateTime<Utc> {
    at.date_naive()
        .and_hms_opt(at.hour() / 6 * 6, 0, 0)
        .expect("a six-hour boundary is a valid UTC time")
        .and_utc()
}
/// Read a public edition by opaque ID. Retention is independent of personal records.
pub fn get(app: &App, key: &str) -> Result<Option<Value>> {
    let row:Option<(String,i64,i64)>=app.store.conn()?.query_row("SELECT body,retain_until,expires_at FROM public_cache WHERE cache_key=? AND kind='edition' AND body IS NOT NULL",[key],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
    row.map(|(body, until, expires)| {
        let mut value: Value = serde_json::from_str(&body)?;
        if until <= Utc::now().timestamp() {
            value["cacheStatus"] = json!("expired");
            value["gaps"]
                .as_array_mut()
                .map(|g| g.push(json!("公共汇编的缓存保留期已过，原始引文可继续核对")));
        } else if syndicated::is_stale(&value, Utc::now())
            || (expires <= Utc::now().timestamp() && !syndicated::current(&value, Utc::now()))
        {
            value["cacheStatus"] = json!("stale");
        } else {
            value["cacheStatus"] = json!("ready");
        }
        Ok(value)
    })
    .transpose()
}
fn previous_edition(app: &App, topic: &str, language: &str) -> Result<Option<Value>> {
    let raw:Option<String>=app.store.conn()?.query_row("SELECT body FROM public_cache WHERE kind='edition' AND body IS NOT NULL AND retain_until>? AND json_extract(body,'$.catalogTopicId')=? AND json_extract(body,'$.language')=? AND json_extract(body,'$.pipelineVersion')=? ORDER BY COALESCE(json_extract(body,'$.editionWindowStart'),json_extract(body,'$.utcDate')) DESC,json_extract(body,'$.createdAt') DESC LIMIT 1",params![Utc::now().timestamp(),topic,language,PIPELINE_VERSION],|r|r.get(0)).optional()?;
    raw.map(|s| serde_json::from_str(&s).map_err(Into::into))
        .transpose()
}
fn result(mut value: Option<Value>, status: &str, warning: Option<&str>) -> EditionResult {
    if let Some(value) = &mut value {
        value["cacheStatus"] = json!(status);
    }
    EditionResult {
        edition: value,
        cache_status: status.into(),
        warning: warning.map(str::to_owned),
    }
}
fn failure_warning(code: &str) -> &'static str {
    match code {
        "no_new_sources" => "本时段未取得新原文，保留上一版与原日期；没有重新调用付费编辑",
        "public_sources_incomplete" => {
            "本次来源覆盖存在缺口，尚未核验到新增资料；保留上一版与原日期"
        }
        "public_outcome_unknown" | "public_idempotency_conflict" | "public_retry_exhausted" => {
            "公共编辑上次调用未能安全确认，已停止自动重放；旧内容保留原日期"
        }
        "public_inputs_expired" | "public_inputs_changed" => {
            "公共编辑的固定输入暂不可用，已停止重新生成；旧内容保留原日期"
        }
        "public_daily_budget_exhausted" => "今日公共编辑预算已用完；旧内容保留原日期",
        "hosted_model_unconfigured" => "公共编辑尚未配置平台模型；旧内容保留原日期",
        "agent_unavailable" | "agent_request_invalid" | "agent_protocol" | "agent_failed" => {
            "日报编辑服务暂时不可用，请稍后重试；旧内容保留原日期"
        }
        "truncated" | "invalid_response" | "response_too_large" => {
            "日报编辑结果不完整，未发布新稿；旧内容保留原日期"
        }
        "network" | "timeout" => "外部服务连接未完成，未发布新稿；旧内容保留原日期",
        code if code.starts_with("editorial_") => {
            "现有公共材料未通过编辑与事实核查，未发布新稿；旧内容保留原日期"
        }
        _ => "获取公开来源暂时失败；保留的旧内容未被当作今天的新进展",
    }
}
pub async fn edition(app: &App, topic_id: &str, language: &str) -> Result<EditionResult> {
    if topic_id == "ai" && language == "zh-CN" && crate::aihot::enabled() {
        return syndicated::edition(app).await;
    }
    edition_at(app, topic_id, language, Utc::now()).await
}
async fn edition_at(
    app: &App,
    topic_id: &str,
    language: &str,
    at: chrono::DateTime<Utc>,
) -> Result<EditionResult> {
    let topic = CATALOG
        .iter()
        .find(|t| t.id == topic_id)
        .ok_or_else(|| Error::bad("只能共享固定公开领域"))?;
    if language != "zh-CN" {
        return Err(Error::bad("公共汇编语言无效"));
    }
    let date = at.date_naive().to_string();
    let window = edition_window(at);
    let key = edition_key(topic.id, &window.to_rfc3339(), language);
    let lease = match acquire(app, &key, "edition")? {
        Reservation::Ready(value) => return Ok(result(Some(value), "ready", None)),
        Reservation::Waiting(value) => {
            let value = value.or(previous_edition(app, topic.id, language)?);
            let error: Option<String> = app.store.conn()?.query_row(
                "SELECT last_error FROM public_cache WHERE cache_key=?",
                [&key],
                |r| r.get(0),
            )?;
            return Ok(result(
                value.clone(),
                if value.is_some() && error.as_deref() == Some("no_new_sources") {
                    "unchanged"
                } else if value.is_some() {
                    "stale"
                } else if error.is_some() {
                    "unavailable"
                } else {
                    "refreshing"
                },
                Some(
                    error
                        .as_deref()
                        .map(failure_warning)
                        .unwrap_or("公共汇编正在刷新"),
                ),
            ));
        }
        Reservation::Refresh(lease) => lease,
    };
    let built = tokio::time::timeout(
        StdDuration::from_secs(720),
        inputs::build(app, topic, language, &lease, &date),
    )
    .await
    .map_err(|_| Error::upstream("timeout", "公共汇编刷新超时"))
    .and_then(|r| r);
    match built {
        Ok(mut value) => {
            value["editionWindowStart"] = json!(window.to_rfc3339());
            value["editionWindowEnd"] = json!((window + Duration::hours(6)).to_rfc3339());
            if let Some(sources) = value["sources"].as_array() {
                let as_of = value["createdAt"]
                    .as_str()
                    .and_then(crate::freshness::publication)
                    .unwrap_or(at);
                let coverage = crate::freshness::coverage_note(sources, as_of);
                value["editorial"]["coverageNote"] = json!(format!(
                    "{coverage} 汇编窗口：{}–{} UTC。",
                    window.format("%Y-%m-%d %H:%M"),
                    (window + Duration::hours(6)).format("%H:%M")
                ));
            }
            if !commit(app, &lease, &value, 6 * 3600, 30 * 86400)? {
                return Ok(result(
                    previous_edition(app, topic.id, language)?,
                    "stale",
                    Some("公共汇编的刷新租约已变更"),
                ));
            }
            Ok(result(Some(value), "ready", None))
        }
        Err(error) => {
            eprintln!("public_edition={} code={}", lease.key, error.code);
            failed(app, &lease, error.code)?;
            let previous = lease
                .previous
                .or(previous_edition(app, topic.id, language)?);
            Ok(result(
                previous.clone(),
                if previous.is_some() && error.code == "no_new_sources" {
                    "unchanged"
                } else if previous.is_some() {
                    "stale"
                } else {
                    "unavailable"
                },
                Some(failure_warning(error.code)),
            ))
        }
    }
}
/// Remove raw pages after 24h. Referenced editions retain their quotations after TTL.
pub fn retention(app: &App) -> Result<()> {
    let at = Utc::now().timestamp();
    let conn = app.store.conn()?;
    inputs::retention(&conn, at)?;
    conn.execute("UPDATE public_model_calls SET response=NULL WHERE status='completed' AND response IS NOT NULL AND updated<=?", [at-86400])?;
    conn.execute(
        "DELETE FROM public_cache WHERE kind='page' AND retain_until<? AND lease_until<?",
        params![at, at],
    )?;
    conn.execute("DELETE FROM public_cache WHERE kind='edition' AND retain_until<? AND lease_until<? AND NOT EXISTS(SELECT 1 FROM records WHERE kind='report' AND (json_extract(data,'$.publicEditionId')=public_cache.cache_key OR EXISTS(SELECT 1 FROM json_each(data,'$.publicEditionIds') WHERE value=public_cache.cache_key)))",params![at,at])?;
    conn.execute("DELETE FROM public_urls WHERE registered_at<? AND url NOT IN (SELECT json_extract(body,'$.url') FROM public_cache WHERE kind='page')",[at-Duration::days(30).num_seconds()])?;
    Ok(())
}
#[cfg(test)]
#[path = "shared_public_tests.rs"]
mod tests;
