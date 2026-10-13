//! Platform-owned public editorial calls. Never contains user IDs or private prompts.
use crate::{
    error::{Error, Result},
    App,
};
use axum::http::StatusCode;
use rusqlite::{params, Connection, OptionalExtension};
pub fn migrate(conn: &Connection) -> Result<()> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS public_model_calls(operation_key TEXT PRIMARY KEY,fingerprint TEXT NOT NULL,status TEXT NOT NULL,response TEXT,attempt INTEGER NOT NULL,created INTEGER NOT NULL,updated INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS public_model_attempts(operation_key TEXT NOT NULL,attempt INTEGER NOT NULL,day TEXT NOT NULL,status TEXT NOT NULL,created INTEGER NOT NULL,PRIMARY KEY(operation_key,attempt));
CREATE INDEX IF NOT EXISTS public_model_day ON public_model_attempts(day);")?;
    Ok(())
}
pub fn daily_limit() -> Result<i64> {
    let raw = std::env::var("MEMHOT_PUBLIC_DAILY_CALL_LIMIT").unwrap_or_else(|_| "20".into());
    raw.parse::<i64>()
        .ok()
        .filter(|n| (0..=200).contains(n))
        .ok_or_else(|| Error::bad("MEMHOT_PUBLIC_DAILY_CALL_LIMIT须为0至200"))
}
pub fn reserve(app: &App, key: &str, fingerprint: &str, limit: i64) -> Result<Option<String>> {
    if key.is_empty() || key.len() > 512 || !(0..=200).contains(&limit) {
        return Err(Error::bad("公共编辑请求参数无效"));
    }
    let at = chrono::Utc::now();
    let timestamp = at.timestamp();
    let day = at.format("%Y-%m-%d").to_string();
    let mut conn = app.store.conn()?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let previous:Option<(String,String,Option<String>,i64)>=tx.query_row("SELECT fingerprint,status,response,attempt FROM public_model_calls WHERE operation_key=?",[key],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
    let attempt = if let Some((old, state, response, attempt)) = previous {
        if old != fingerprint {
            return Err(Error::new(
                StatusCode::CONFLICT,
                "public_idempotency_conflict",
                "公共编辑阶段的输入已变化，不能复用原调用ID",
            ));
        }
        if state == "completed" {
            return response.map(Some).ok_or_else(|| {
                Error::upstream(
                    "public_outcome_unknown",
                    "已完成公共编辑的缓存不可用，已阻止重复调用",
                )
            });
        }
        if state != "rejected" {
            return Err(Error::upstream(
                "public_outcome_unknown",
                "公共编辑上次调用结果不明，已阻止自动重复调用",
            ));
        }
        if attempt >= 2 {
            return Err(Error::new(
                StatusCode::CONFLICT,
                "public_retry_exhausted",
                "此公共编辑阶段已达到重试上限",
            ));
        }
        attempt + 1
    } else {
        1
    };
    let used: i64 = tx.query_row(
        "SELECT count(*) FROM public_model_attempts WHERE day=?",
        [&day],
        |r| r.get(0),
    )?;
    if used >= limit {
        return Err(Error::new(
            StatusCode::TOO_MANY_REQUESTS,
            "public_daily_budget_exhausted",
            "今日公共编辑调用预算已用完",
        ));
    }
    tx.execute("INSERT INTO public_model_calls(operation_key,fingerprint,status,attempt,created,updated) VALUES(?,?,'pending',?,?,?) ON CONFLICT(operation_key) DO UPDATE SET status='pending',response=NULL,attempt=excluded.attempt,updated=excluded.updated",params![key,fingerprint,attempt,timestamp,timestamp])?;
    tx.execute("INSERT INTO public_model_attempts(operation_key,attempt,day,status,created) VALUES(?,?,?,'pending',?)",params![key,attempt,day,timestamp])?;
    tx.commit()?;
    Ok(None)
}
pub fn settle(app: &App, key: &str, result: &Result<String>) -> Result<()> {
    let mut conn = app.store.conn()?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let previous: Option<(String, i64)> = tx
        .query_row(
            "SELECT status,attempt FROM public_model_calls WHERE operation_key=?",
            [key],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let (state, attempt) = previous.ok_or_else(Error::missing)?;
    if state != "pending" {
        return Ok(());
    }
    let status = match result {
        Ok(_) => "completed",
        Err(e)
            if matches!(
                e.code,
                "invalid_input"
                    | "model_unconfigured"
                    | "provider_auth"
                    | "provider_rejected"
                    | "rate_limited"
            ) =>
        {
            "rejected"
        }
        // Truncated or malformed responses can still be billable. Never auto-repeat.
        Err(_) => "unknown",
    };
    tx.execute("UPDATE public_model_calls SET status=?,response=?,updated=? WHERE operation_key=? AND status='pending'",params![status,result.as_ref().ok(),chrono::Utc::now().timestamp(),key])?;
    tx.execute(
        "UPDATE public_model_attempts SET status=? WHERE operation_key=? AND attempt=?",
        params![status, key, attempt],
    )?;
    tx.commit()?;
    Ok(())
}
