//! A day references an existing thought; edits and model merges never move it between days.
use crate::{
    auth::User,
    error::{Error, Result},
    persistence, App,
};
use axum::{
    extract::{Path, State},
    Json,
};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

pub(crate) fn timezone(conn: &Connection, user: &str) -> Result<chrono_tz::Tz> {
    let raw: String =
        conn.query_row("SELECT profile FROM users WHERE id=?", [user], |r| r.get(0))?;
    let profile: Value = serde_json::from_str(&raw)?;
    Ok(profile["timezone"]
        .as_str()
        .unwrap_or("Asia/Shanghai")
        .parse()
        .unwrap_or(chrono_tz::Asia::Shanghai))
}
pub(crate) fn today(conn: &Connection, user: &str) -> Result<String> {
    Ok(Utc::now()
        .with_timezone(&timezone(conn, user)?)
        .date_naive()
        .to_string())
}
pub(crate) fn local_date(value: &Value, tz: chrono_tz::Tz) -> Option<String> {
    ["occurredAt", "createdAt"]
        .into_iter()
        .filter_map(|field| value[field].as_str())
        .find_map(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.with_timezone(&tz).date_naive().to_string())
}
pub(crate) fn initialize(conn: &Connection, user: &str, thought: &Value) -> Result<()> {
    let key = thought["id"].as_str().ok_or_else(Error::internal)?;
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM thought_days WHERE user_id=? AND thought_id=?)",
        params![user, key],
        |r| r.get(0),
    )?;
    if exists {
        return Ok(());
    }
    let tz = timezone(conn, user)?;
    let mut source_ids = thought["sourceIds"].as_array().cloned().unwrap_or_default();
    if thought["sourceId"].is_string() {
        source_ids.push(thought["sourceId"].clone());
    }
    let mut dates = Vec::new();
    for source in source_ids.iter().filter_map(Value::as_str) {
        let raw: Option<String> = conn
            .query_row(
                "SELECT data FROM records WHERE user_id=? AND kind='source' AND id=?",
                params![user, source],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(raw) = raw {
            if let Some(date) = local_date(&serde_json::from_str::<Value>(&raw)?, tz) {
                dates.push(date);
            }
        }
    }
    dates.sort();
    let date = dates
        .into_iter()
        .next()
        .or_else(|| local_date(thought, tz))
        .unwrap_or_else(|| "1970-01-01".to_owned());
    conn.execute("INSERT OR IGNORE INTO thought_days(user_id,thought_id,local_date,reason,created) VALUES(?,?,?,'origin',?)",params![user,key,date,crate::now()])?;
    Ok(())
}
pub(crate) fn migrate(conn: &Connection) -> Result<()> {
    let tx = rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)?;
    let exists: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='thought_days')",
        [],
        |r| r.get(0),
    )?;
    if !exists {
        tx.execute_batch("CREATE TABLE thought_days(user_id TEXT NOT NULL REFERENCES users(id),thought_id TEXT NOT NULL,local_date TEXT NOT NULL,reason TEXT NOT NULL,created TEXT NOT NULL,PRIMARY KEY(user_id,thought_id,local_date)); CREATE INDEX thought_days_date ON thought_days(user_id,local_date,thought_id);")?;
        let rows = {
            let mut stmt = tx.prepare("SELECT user_id,data FROM records WHERE kind='thought'")?;
            let rows =
                stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };
        for (user, raw) in rows {
            initialize(&tx, &user, &serde_json::from_str::<Value>(&raw)?)?;
        }
    }
    // Install on existing schemas too; every hard-delete path shares this tenant-bound cleanup.
    tx.execute_batch("CREATE TRIGGER IF NOT EXISTS thought_days_delete AFTER DELETE ON records WHEN OLD.kind='thought' BEGIN DELETE FROM thought_days WHERE user_id=OLD.user_id AND thought_id=OLD.id; END;
        DELETE FROM thought_days WHERE NOT EXISTS(SELECT 1 FROM records WHERE records.user_id=thought_days.user_id AND records.kind='thought' AND records.id=thought_days.thought_id);")?;
    tx.commit()?;
    Ok(())
}
pub(crate) fn active(value: &Value) -> bool {
    matches!(value["status"].as_str(), None | Some("open" | "kept"))
        && !value["mergedInto"].is_string()
}
pub(crate) fn contains(conn: &Connection, user: &str, key: &str, date: &str) -> Result<bool> {
    Ok(conn.query_row("SELECT EXISTS(SELECT 1 FROM thought_days WHERE user_id=? AND thought_id=? AND local_date=?)",params![user,key,date],|r|r.get(0))?)
}
pub(crate) fn annotate(
    conn: &Connection,
    user: &str,
    value: &mut Value,
    matched: Option<&str>,
) -> Result<()> {
    let date = today(conn, user)?;
    let key = value["id"].as_str().ok_or_else(Error::internal)?;
    let in_today = active(value) && contains(conn, user, key, &date)?;
    let latest: Option<String> = conn.query_row(
        "SELECT MAX(local_date) FROM thought_days WHERE user_id=? AND thought_id=?",
        params![user, key],
        |r| r.get(0),
    )?;
    value["thinkingDate"] = json!(matched.map(str::to_owned).or(latest));
    value["inToday"] = json!(in_today);
    Ok(())
}
pub(crate) fn for_date(conn: &Connection, user: &str, date: &str) -> Result<Vec<Value>> {
    for_date_limited(conn, user, date, None)
}
pub(crate) fn for_date_limited(
    conn: &Connection,
    user: &str,
    date: &str,
    limit: Option<u32>,
) -> Result<Vec<Value>> {
    let mut stmt = conn.prepare("SELECT r.data FROM records r JOIN thought_days d ON d.user_id=r.user_id AND d.thought_id=r.id WHERE r.user_id=? AND r.kind='thought' AND d.local_date=? AND COALESCE(json_extract(r.data,'$.status'),'open') IN ('open','kept') AND json_extract(r.data,'$.mergedInto') IS NULL ORDER BY r.created DESC,r.id DESC LIMIT ?")?;
    let rows = stmt.query_map(
        params![user, date, limit.map(i64::from).unwrap_or(-1)],
        |r| r.get::<_, String>(0),
    )?;
    let mut values = Vec::new();
    for row in rows {
        let mut value: Value = serde_json::from_str(&row?)?;
        annotate(conn, user, &mut value, Some(date))?;
        values.push(value);
    }
    Ok(values)
}

pub async fn get(
    State(app): State<App>,
    User(user): User,
    Path(key): Path<String>,
) -> Result<Json<Value>> {
    let mut value = app.store.get(&user, "thought", &key)?;
    if value["status"] == "deleted" || value["mergedInto"].is_string() {
        return Err(Error::missing());
    }
    annotate(&*app.store.conn()?, &user, &mut value, None)?;
    Ok(Json(value))
}
pub async fn carry(
    State(app): State<App>,
    User(user): User,
    Path(key): Path<String>,
    Json(input): Json<Value>,
) -> Result<Json<Value>> {
    let map = input.as_object().ok_or_else(|| Error::bad("请求无效"))?;
    if map.len() != 1 || input["revision"].as_u64().is_none() {
        return Err(Error::bad("仅接受当前版本 revision"));
    }
    let mut conn = app.store.conn()?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let mut value =
        persistence::current(&tx, &user, "thought", &key)?.ok_or_else(Error::missing)?;
    if value["status"] == "deleted" || value["mergedInto"].is_string() {
        return Err(Error::missing());
    }
    let date = today(&tx, &user)?;
    let already = contains(&tx, &user, &key, &date)? && active(&value);
    // The immediately repeated carry is idempotent even after it restored an archived thought.
    if already && input["revision"].as_u64() != Some(value["revision"].as_u64().unwrap_or(1)) {
        let replay: bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM thought_days WHERE user_id=? AND thought_id=? AND local_date=? AND reason=?)",params![user,key,date,format!("explicit:{}",input["revision"])],|r|r.get(0))?;
        if !replay {
            return Err(crate::topics::conflict());
        }
    }
    if !already {
        crate::topics::check_revision(&value, &input)?;
        if !active(&value) {
            value["status"] = json!("kept");
            value["researchEligible"] = json!(true);
            value["revision"] = json!(value["revision"].as_u64().unwrap_or(1) + 1);
            value["updatedAt"] = json!(crate::now());
            persistence::put(&tx, &user, "thought", &value)?;
        }
        tx.execute("INSERT INTO thought_days(user_id,thought_id,local_date,reason,created) VALUES(?,?,?,?,?) ON CONFLICT(user_id,thought_id,local_date) DO UPDATE SET reason=excluded.reason",params![user,key,date,format!("explicit:{}",input["revision"]),crate::now()])?;
        crate::daily::enqueue_today(&tx, &user)?;
    }
    annotate(&tx, &user, &mut value, Some(&date))?;
    tx.commit()?;
    Ok(Json(value))
}
