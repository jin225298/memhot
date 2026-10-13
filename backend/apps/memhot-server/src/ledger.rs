//! Durable request receipts and atomic quota reservations. Unknown outcomes stay held.
use crate::{
    billing,
    db::Job,
    error::{Error, Result},
    App,
};
use rusqlite::{params, OptionalExtension};
pub fn reserve(app: &App, job: &Job, key: &str) -> Result<Option<String>> {
    reserve_request(app, &job.user, Some(job), key, "text", 1)
}
pub fn reserve_units(
    app: &App,
    job: &Job,
    key: &str,
    kind: &str,
    units: i64,
) -> Result<Option<String>> {
    reserve_request(app, &job.user, Some(job), key, kind, units)
}
pub fn reserve_request(
    app: &App,
    user: &str,
    job: Option<&Job>,
    key: &str,
    kind: &str,
    units: i64,
) -> Result<Option<String>> {
    reserve_fingerprinted(app, user, job, key, key, kind, units)
}
pub fn reserve_fingerprinted(
    app: &App,
    user: &str,
    job: Option<&Job>,
    key: &str,
    fingerprint: &str,
    kind: &str,
    units: i64,
) -> Result<Option<String>> {
    if key.is_empty() || key.len() > 512 {
        return Err(Error::bad("请求ID无效"));
    }
    let mut conn = app.store.conn()?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    if let Some(job) = job {
        if user != job.user {
            return Err(Error::missing());
        }
        let active:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM jobs WHERE id=? AND user_id=? AND status='running' AND attempt=?)",params![job.id,user,job.attempt],|r|r.get(0))?;
        if !active {
            return Err(Error::missing());
        }
        let unsettled:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM receipts WHERE user_id=? AND job_id=? AND key!=? AND status IN ('pending','paid_incomplete'))",params![user,job.id,key],|r|r.get(0))?;
        if unsettled {
            return Err(Error::upstream(
                "outcome_unknown",
                "当前任务已有结果不明的请求，已阻止改配置后自动重复调用",
            ));
        }
    }
    let old_fingerprint: Option<(String, String, i64)> = tx
        .query_row(
            "SELECT fingerprint,kind,units FROM billing_usage WHERE user_id=? AND operation_key=?",
            params![user, key],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    if old_fingerprint.is_some_and(|(old, old_kind, old_units)| {
        old != fingerprint || old_kind != kind || old_units != units
    }) {
        return Err(Error::new(
            axum::http::StatusCode::CONFLICT,
            "idempotency_conflict",
            "相同请求ID已用于不同内容",
        ));
    }
    let old: Option<(String, Option<String>)> = tx
        .query_row(
            "SELECT status,response FROM receipts WHERE user_id=? AND key=?",
            params![user, key],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    if let Some((status, content)) = old {
        if status == "completed" {
            return content.map(Some).ok_or_else(|| {
                Error::upstream(
                    "outcome_unknown",
                    "已完成请求的内容已按保留策略清理，已阻止重复调用",
                )
            });
        }
        if status == "paid_incomplete" {
            return Err(Error::upstream(
                "truncated",
                "上次响应不完整，已阻止自动重复调用；请明确重试",
            ));
        }
        return Err(Error::upstream(
            "outcome_unknown",
            "上次请求结果不明，额度保持预占；已阻止自动重复调用",
        ));
    }
    billing::reserve_tx(&tx, user, key, fingerprint, kind, units)?;
    tx.execute(
        "INSERT INTO receipts(user_id,job_id,key,status,created) VALUES(?,?,?,'pending',?)",
        params![
            user,
            job.map(|j| j.id.clone())
                .unwrap_or_else(|| format!("interactive:{key}")),
            key,
            chrono::Utc::now().timestamp()
        ],
    )?;
    tx.commit()?;
    Ok(None)
}
pub fn settle(app: &App, job: &Job, key: &str, result: &Result<String>) -> Result<()> {
    settle_request(app, &job.user, key, result)
}
pub fn settle_request(app: &App, user: &str, key: &str, result: &Result<String>) -> Result<()> {
    let mut conn = app.store.conn()?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let terminal:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM billing_usage WHERE user_id=? AND operation_key=? AND state IN ('spent','released'))",params![user,key],|r|r.get(0))?;
    if terminal {
        return Ok(());
    }
    match result {
        Ok(text) => {
            billing::settle_tx(&tx, user, key, "spent")?;
            tx.execute(
                "UPDATE receipts SET status='completed',response=? WHERE user_id=? AND key=?",
                params![text, user, key],
            )?;
        }
        Err(e)
            if matches!(
                e.code,
                "truncated" | "invalid_response" | "response_too_large"
            ) =>
        {
            billing::settle_tx(&tx, user, key, "spent")?;
            tx.execute("UPDATE receipts SET status='paid_incomplete',response=NULL WHERE user_id=? AND key=?",params![user,key])?;
        }
        Err(e)
            if matches!(
                e.code,
                "invalid_input"
                    | "model_unconfigured"
                    | "provider_auth"
                    | "provider_rejected"
                    | "rate_limited"
                    | "upstream"
                    | "asr_failed"
                    | "busy"
            ) =>
        {
            billing::settle_tx(&tx, user, key, "released")?;
            tx.execute(
                "DELETE FROM receipts WHERE user_id=? AND key=?",
                params![user, key],
            )?;
        }
        Err(_) => {
            billing::settle_tx(&tx, user, key, "unknown")?;
        }
    }
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{auth, id};
    use serde_json::json;
    #[test]
    fn receipts_are_tenant_scoped_replay_success_and_stop_unknown_outcomes() {
        let dir = std::env::temp_dir().join(format!("memhot-ledger-{}", id()));
        let app = App::open(dir.clone()).unwrap();
        let (a, _) = auth::create_pair(&app, "A", None).unwrap();
        let (b, _) = auth::create_pair(&app, "B", None).unwrap();
        app.store.queue(&a, "formation", json!({}), "a").unwrap();
        let ja = app.store.claim().unwrap().unwrap();
        app.store.queue(&b, "formation", json!({}), "b").unwrap();
        let jb = app.store.claim().unwrap().unwrap();
        assert!(reserve(&app, &ja, "same-key").unwrap().is_none());
        assert!(reserve(&app, &jb, "same-key").unwrap().is_none());
        settle(&app, &ja, "same-key", &Ok("transcript".into())).unwrap();
        assert_eq!(
            reserve(&app, &ja, "same-key").unwrap(),
            Some("transcript".into())
        );
        settle(
            &app,
            &jb,
            "same-key",
            &Err(Error::upstream("timeout", "timed out")),
        )
        .unwrap();
        assert_eq!(
            reserve(&app, &jb, "same-key").unwrap_err().code,
            "outcome_unknown"
        );
        app.store
            .conn()
            .unwrap()
            .execute("DELETE FROM jobs WHERE id=?", [&ja.id])
            .unwrap();
        assert_eq!(
            reserve(&app, &ja, "new-request").unwrap_err().code,
            "not_found"
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
