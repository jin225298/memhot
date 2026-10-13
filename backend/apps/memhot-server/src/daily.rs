//! Daily summaries are materialized topic sections with auditable source coverage.
use crate::{
    auth::User,
    db::Job,
    error::{Error, Result},
    now, App,
};
use axum::{extract::State, Json};
use chrono::{DateTime, Utc};
use rusqlite::{params, Transaction};
use serde_json::{json, Value};
fn timezone(profile: &Value) -> chrono_tz::Tz {
    profile["timezone"]
        .as_str()
        .unwrap_or("Asia/Shanghai")
        .parse()
        .unwrap_or(chrono_tz::Asia::Shanghai)
}
fn local_date(value: &Value, tz: chrono_tz::Tz) -> Option<String> {
    value["occurredAt"]
        .as_str()
        .or_else(|| value["createdAt"].as_str())
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|at| at.with_timezone(&tz).date_naive().to_string())
}
fn profile(tx: &Transaction<'_>, user: &str) -> Result<Value> {
    let raw: String = tx.query_row("SELECT profile FROM users WHERE id=?", [user], |r| r.get(0))?;
    Ok(serde_json::from_str(&raw)?)
}
fn enqueue(tx: &Transaction<'_>, user: &str, date: &str, reason: &str) -> Result<()> {
    tx.execute("INSERT OR IGNORE INTO jobs(id,user_id,kind,payload,status,created,dedup) VALUES(?,?,'daily_summary',?,'queued',?,?)",
        params![crate::id(),user,json!({"localDate":date}).to_string(),now(),format!("summary:{date}:{reason}")])?;
    Ok(())
}
pub fn enqueue_source(tx: &Transaction<'_>, user: &str, source: &Value) -> Result<()> {
    let tz = timezone(&profile(tx, user)?);
    let date = local_date(source, tz)
        .unwrap_or_else(|| Utc::now().with_timezone(&tz).date_naive().to_string());
    enqueue(
        tx,
        user,
        &date,
        &crate::hash(
            format!(
                "{}:{}:{}",
                source["id"], source["analysisCoverage"], source["topicIds"]
            )
            .as_bytes(),
        ),
    )
}
pub(crate) fn rebuild_source_date(tx: &Transaction<'_>, user: &str, source: &Value) -> Result<()> {
    let tz = timezone(&profile(tx, user)?);
    let date = local_date(source, tz)
        .unwrap_or_else(|| Utc::now().with_timezone(&tz).date_naive().to_string());
    enqueue(tx, user, &date, &crate::id())
}
pub fn enqueue_today(tx: &Transaction<'_>, user: &str) -> Result<()> {
    let tz = timezone(&profile(tx, user)?);
    enqueue(
        tx,
        user,
        &Utc::now().with_timezone(&tz).date_naive().to_string(),
        &crate::id(),
    )
}
pub async fn list(State(app): State<App>, User(user): User) -> Result<Json<Value>> {
    Ok(Json(
        json!({"items":app.store.list(&user,"daily_summary")?}),
    ))
}
pub async fn run(app: &App, job: &Job) -> Result<Vec<(&'static str, Value)>> {
    let p = app.store.profile(&job.user)?;
    let tz = timezone(&p);
    let date = job.payload["localDate"]
        .as_str()
        .ok_or_else(Error::missing)?;
    let sources: Vec<Value> = app
        .store
        .list_all(&job.user, "source")?
        .into_iter()
        .filter(|s| local_date(s, tz).as_deref() == Some(date))
        .collect();
    let ids: Vec<Value> = sources.iter().map(|s| s["id"].clone()).collect();
    let mut sections = Vec::new();
    for t in app.store.list_all(&job.user, "thought")? {
        if matches!(t["status"].as_str(), Some("dismissed" | "resolved"))
            || t["mergedInto"].is_string()
        {
            continue;
        }
        let mut refs = t["sourceIds"].as_array().cloned().unwrap_or_default();
        if t["sourceId"].is_string() && !refs.contains(&t["sourceId"]) {
            refs.push(t["sourceId"].clone());
        }
        refs.retain(|id| ids.contains(id));
        if refs.is_empty()
            && !(t["origin"] == "manual" && local_date(&t, tz).as_deref() == Some(date))
        {
            continue;
        }
        let mut notes = Vec::new();
        for evidence in t["evidenceRefs"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|e| refs.contains(&e["sourceId"]))
        {
            if let Some(note) = evidence["summary"]
                .as_str()
                .or_else(|| evidence["quote"].as_str())
            {
                if !notes.contains(&note) {
                    notes.push(note);
                }
            }
        }
        let summary = if notes.is_empty() {
            t["summary"]
                .as_str()
                .filter(|s| !s.is_empty())
                .or_else(|| t["question"].as_str())
                .unwrap_or_default()
                .to_owned()
        } else {
            notes.join("；")
        };
        sections.push(json!({"topicId":t["id"],"title":t["title"],"summary":summary,"questions":t["question"].as_str().filter(|s|!s.is_empty()).into_iter().collect::<Vec<_>>(),"sourceIds":refs}));
    }
    let total: usize = sources
        .iter()
        .map(|s| {
            s["analysisCoverage"]["totalChars"]
                .as_u64()
                .map(|n| n as usize)
                .unwrap_or_else(|| s["text"].as_str().unwrap_or_default().chars().count())
        })
        .sum();
    let included: u64 = sources
        .iter()
        .map(|s| s["analysisCoverage"]["includedChars"].as_u64().unwrap_or(0))
        .sum();
    let complete = sources
        .iter()
        .all(|s| s["analysisCoverage"]["complete"] == true || s["status"] == "no_speech");
    let value = json!({"id":format!("daily:{date}"),"localDate":date,"timezone":tz.name(),"sections":sections,"sourceIds":ids,"coverage":{"includedChars":included,"totalChars":total,"complete":complete},"status":if complete{"ready"}else{"partial"},"createdAt":now(),"updatedAt":now()});
    Ok(vec![("daily_summary", value)])
}
