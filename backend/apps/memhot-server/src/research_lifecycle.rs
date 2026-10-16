//! Archiving pauses only this tenant's thought research, without erasing paid receipts.
use crate::{
    db::Job,
    error::{Error, Result},
    App,
};
use chrono::Timelike;
use rusqlite::{params, Connection, Transaction};
use serde_json::Value;

pub(crate) fn thought_active(conn: &Connection, user: &str, thought: &str) -> Result<bool> {
    Ok(conn.query_row("SELECT EXISTS(SELECT 1 FROM records WHERE user_id=? AND kind='thought' AND id=? AND COALESCE(json_extract(data,'$.status'),'open') IN ('open','kept') AND json_extract(data,'$.mergedInto') IS NULL)",
        params![user,thought],|r|r.get(0))?)
}

pub(crate) fn job_active(conn: &Connection, job: &Job) -> Result<bool> {
    let running: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM jobs WHERE id=? AND user_id=? AND status='running' AND attempt=? AND lease_until>=?)",
        params![job.id,job.user,job.attempt,chrono::Utc::now().timestamp()],|r|r.get(0))?;
    if !running {
        return Ok(false);
    }
    if job.kind == "research" {
        if let Some(thought) = job.payload["thoughtId"].as_str() {
            return Ok(thought_active(conn, &job.user, thought)?
                && scheduled_applicable(conn, job, chrono::Utc::now())?);
        }
    }
    Ok(true)
}

pub(crate) fn check_active(app: &App, job: &Job) -> Result<()> {
    if !job_active(&*app.store.conn()?, job)? {
        return Err(Error::missing());
    }
    Ok(())
}

pub(crate) fn cancel_archived(tx: &Transaction<'_>, user: &str, thought: &Value) -> Result<()> {
    if !matches!(thought["status"].as_str(), Some("dismissed" | "resolved")) {
        return Ok(());
    }
    tx.execute("UPDATE jobs SET status='cancelled',lease_until=0,message='思考已归档，研究已暂停' WHERE user_id=? AND kind='research' AND json_extract(payload,'$.thoughtId')=? AND status IN ('queued','running','blocked')",
        params![user,thought["id"].as_str()])?;
    // Keep receipts and quota reservations: cancellation never authorizes replay of a paid call.
    Ok(())
}

/// Read fresh payload: a concurrent explicit refresh may have adopted a scheduled job.
pub(crate) fn scheduled_applicable(
    conn: &Connection,
    job: &Job,
    at: chrono::DateTime<chrono::Utc>,
) -> Result<bool> {
    if job.kind != "research" {
        return Ok(true);
    }
    let raw: String = conn.query_row(
        "SELECT payload FROM jobs WHERE id=? AND user_id=?",
        params![job.id, job.user],
        |r| r.get(0),
    )?;
    let payload: Value = serde_json::from_str(&raw)?;
    if payload["deliverImmediately"] == true || payload["retryOf"].is_string() {
        return Ok(true);
    }
    let Some(date) = payload["researchDate"]
        .as_str()
        .or_else(|| payload["date"].as_str())
    else {
        return Ok(true);
    };
    let tz = crate::thought_days::timezone(conn, &job.user)?;
    let night: u32 = conn.query_row(
        "SELECT COALESCE(json_extract(profile,'$.researchHour'),23) FROM users WHERE id=?",
        [&job.user],
        |r| r.get(0),
    )?;
    let local = at.with_timezone(&tz);
    let applicable = if local.hour() >= night {
        local.date_naive()
    } else {
        local.date_naive().pred_opt().ok_or_else(Error::internal)?
    };
    let Some(thought) = payload["thoughtId"].as_str() else {
        return Ok(false);
    };
    Ok(date == applicable.to_string()
        && crate::thought_days::contains(conn, &job.user, thought, date)?)
}
pub(crate) fn cancel_stale(
    conn: &Connection,
    user: &str,
    at: chrono::DateTime<chrono::Utc>,
) -> Result<()> {
    let jobs = {
        let mut stmt=conn.prepare("SELECT id,payload,attempt FROM jobs WHERE user_id=? AND kind='research' AND status='queued'")?;
        let rows = stmt.query_map([user], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
    };
    for (id, raw, attempt) in jobs {
        let job = Job {
            id,
            user: user.to_owned(),
            kind: "research".into(),
            payload: serde_json::from_str(&raw)?,
            attempt,
        };
        if !scheduled_applicable(conn, &job, at)? {
            // Never delete paid receipts, reservations or content dedup keys.
            conn.execute("UPDATE jobs SET status='cancelled',lease_until=0,payload=json_set(payload,'$.expiredSchedule',json('true')),message='研究日期已过期，已停止自动处理' WHERE id=? AND user_id=? AND status='queued'",params![job.id,user])?;
        }
    }
    Ok(())
}

/// Only an unclaimed expired schedule can be safely rearmed without a billing-to-job key.
/// Attempted jobs stay manual-only because retention can remove their receipts.
pub(crate) fn queue_scheduled(app: &App, user: &str, payload: Value, dedup: &str) -> Result<()> {
    let mut conn = app.store.conn()?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    tx.execute("INSERT OR IGNORE INTO jobs(id,user_id,kind,payload,status,created,dedup) VALUES(?,?,'research',?,'queued',?,?)",params![crate::id(),user,payload.to_string(),crate::now(),dedup])?;
    // operation_key is hashed, so additionally require no tenant usage since creation.
    // This intentionally prefers manual recovery to risking an uncertain paid replay.
    tx.execute(
        "UPDATE jobs SET status='queued',payload=?3,lease_until=0,next_at=0,message=NULL
        WHERE user_id=?1 AND dedup=?2 AND kind='research' AND status='cancelled' AND attempt=0
        AND json_extract(payload,'$.expiredSchedule')=1
        AND json_extract(payload,'$.researchDate')!=json_extract(?3,'$.researchDate')
        AND EXISTS(SELECT 1 FROM thought_days d WHERE d.user_id=jobs.user_id
            AND d.thought_id=json_extract(?3,'$.thoughtId')
            AND d.local_date=json_extract(?3,'$.researchDate') AND d.reason LIKE 'explicit:%')
        AND EXISTS(SELECT 1 FROM records t WHERE t.user_id=jobs.user_id AND t.kind='thought'
            AND t.id=json_extract(?3,'$.thoughtId') AND COALESCE(json_extract(t.data,'$.status'),'open') IN ('open','kept')
            AND json_extract(t.data,'$.mergedInto') IS NULL AND COALESCE(json_extract(t.data,'$.researchEligible'),1)=1)
        AND NOT EXISTS(SELECT 1 FROM receipts r WHERE r.user_id=jobs.user_id AND r.job_id=jobs.id)
        AND NOT EXISTS(SELECT 1 FROM billing_usage b WHERE b.user_id=jobs.user_id
            AND b.created>=COALESCE(CAST(strftime('%s',jobs.created) AS INTEGER),0))",
        params![user, dedup, payload.to_string()],
    )?;
    tx.commit()?;
    Ok(())
}
