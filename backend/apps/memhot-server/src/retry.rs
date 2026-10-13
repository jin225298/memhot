//! Explicit bounded recovery; never silently repeat an uncertain paid request.
use crate::{
    auth::User,
    error::{Error, Result},
    id, now, App,
};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Retry {
    #[serde(default)]
    accept_duplicate_charge: bool,
}
pub async fn retry(
    State(app): State<App>,
    User(user): User,
    Path(key): Path<String>,
    Json(input): Json<Retry>,
) -> Result<(StatusCode, Json<Value>)> {
    let mut conn = app.store.conn()?;
    let tx = conn.transaction()?;
    let old: Option<(String, String, String)> = tx
        .query_row(
            "SELECT kind,payload,status FROM jobs WHERE id=? AND user_id=?",
            params![key, user],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let (kind, raw, status) = old.ok_or_else(Error::missing)?;
    if status != "failed" {
        return Err(Error::new(
            StatusCode::CONFLICT,
            "job_not_failed",
            "只能重建已失败的任务",
        ));
    }
    let dedup = format!("retry:{key}");
    let existing: Option<Value> = tx
        .query_row(
            "SELECT id,status FROM jobs WHERE user_id=? AND dedup=?",
            params![user, dedup],
            |r| Ok(json!({"id":r.get::<_,String>(0)?,"status":r.get::<_,String>(1)?})),
        )
        .optional()?;
    if let Some(existing) = existing {
        return Ok((StatusCode::ACCEPTED, Json(existing)));
    }
    let requires:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM receipts WHERE user_id=? AND job_id=? AND status IN ('pending','paid_incomplete'))",params![user,key],|r|r.get(0))?;
    if requires && !input.accept_duplicate_charge {
        return Err(Error::new(
            StatusCode::CONFLICT,
            "duplication_confirmation_required",
            "上次请求可能已计费；重试可能再次收费，请明确确认",
        ));
    }
    let mut payload: Value = serde_json::from_str(&raw)?;
    if !matches!(
        kind.as_str(),
        "transcribe" | "formation" | "research" | "digest" | "material_translation"
    ) || payload.as_object().is_none_or(|m| m.is_empty())
    {
        return Err(Error::new(
            StatusCode::CONFLICT,
            "retry_expired",
            "任务上下文已过期，请重新创建",
        ));
    }
    let source = if matches!(kind.as_str(), "transcribe" | "formation") {
        let source = payload["sourceId"].as_str().ok_or_else(Error::missing)?;
        let raw: Option<String> = tx
            .query_row(
                "SELECT data FROM records WHERE user_id=? AND kind='source' AND id=?",
                params![user, source],
                |r| r.get(0),
            )
            .optional()?;
        Some(serde_json::from_str::<Value>(
            &raw.ok_or_else(Error::missing)?,
        )?)
    } else {
        None
    };
    if let Some(thought) = payload["thoughtId"].as_str() {
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM records WHERE user_id=? AND kind='thought' AND id=?)",
            params![user, thought],
            |r| r.get(0),
        )?;
        if !exists {
            return Err(Error::missing());
        }
    }
    if kind == "transcribe" {
        let source = source.as_ref().ok_or_else(Error::missing)?;
        let filename = source["_audioPath"]
            .as_str()
            .filter(|s| !s.contains('/') && !s.contains('\\'))
            .ok_or_else(Error::missing)?;
        if !app.data_dir.join("audio").join(filename).is_file() {
            return Err(Error::new(
                StatusCode::CONFLICT,
                "audio_expired",
                "原始音频已过期或删除，无法重试",
            ));
        }
    }
    let queued: i64 = tx.query_row(
        "SELECT count(*) FROM jobs WHERE user_id=? AND status IN ('queued','running')",
        [&user],
        |r| r.get(0),
    )?;
    let recent:i64=tx.query_row("SELECT count(*) FROM jobs WHERE user_id=? AND dedup LIKE 'retry:%' AND julianday(created)>julianday('now')-1",[&user],|r|r.get(0))?;
    if queued >= 8 || recent >= 20 {
        return Err(Error::new(
            StatusCode::TOO_MANY_REQUESTS,
            "retry_limit",
            "待处理任务或今日手动重试已达上限，请稍后重试",
        ));
    }
    let new = id();
    payload["retryOf"] = json!(key);
    tx.execute("INSERT INTO jobs(id,user_id,kind,payload,status,created,dedup) VALUES(?,?,?,?,'queued',?,?)",params![new,user,kind,payload.to_string(),now(),dedup])?;
    if let Some(source) = source {
        if kind == "transcribe" {
            tx.execute("UPDATE records SET data=json_set(data,'$.status','queued','$.error',NULL) WHERE user_id=? AND kind='source' AND id=?",params![user,source["id"].as_str()])?;
        }
    }
    tx.commit()?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({"id":new,"status":"queued"})),
    ))
}
