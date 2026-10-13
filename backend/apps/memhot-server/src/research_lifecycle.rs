//! Archiving pauses only this tenant's thought research, without erasing paid receipts.
use crate::{
    db::Job,
    error::{Error, Result},
    App,
};
use rusqlite::{params, Connection, Transaction};
use serde_json::Value;

pub(crate) fn thought_active(conn: &Connection, user: &str, thought: &str) -> Result<bool> {
    Ok(conn.query_row("SELECT EXISTS(SELECT 1 FROM records WHERE user_id=? AND kind='thought' AND id=? AND COALESCE(json_extract(data,'$.status'),'open') IN ('open','kept'))",
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
            return thought_active(conn, &job.user, thought);
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
