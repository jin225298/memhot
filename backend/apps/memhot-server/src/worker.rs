use crate::{
    audio,
    db::Job,
    error::{Error, Result},
    research, sources, App,
};
use chrono::{TimeZone, Timelike};
use rusqlite::params;
use serde_json::json;
use std::time::Duration;

pub async fn run(app: App) {
    let mut tick = tokio::time::interval(Duration::from_secs(2));
    let mut maintenance = tokio::time::interval(Duration::from_secs(60));
    let mut running = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            _=maintenance.tick()=>{
                if let Err(e)=schedule(&app){eprintln!("schedule error code={}",e.code);}
                if let Err(e)=retention(&app){eprintln!("retention error code={}",e.code);}
            }
            _=tick.tick()=>{},
            _=running.join_next(),if !running.is_empty()=>{},
        }
        while running.len() < 2 {
            let job = match app.store.claim() {
                Ok(Some(job)) => job,
                Ok(None) => break,
                Err(e) => {
                    eprintln!("claim error code={}", e.code);
                    break;
                }
            };
            let app = app.clone();
            running.spawn(async move {
                let result = tokio::time::timeout(Duration::from_secs(840), execute(&app, &job))
                    .await
                    .unwrap_or_else(|_| Err(Error::upstream("timeout", "任务达到执行时限")));
                if let Err(error) = result {
                    let _ = app.store.fail(&job, &error);
                    mark_source_error(&app, &job, &error);
                    eprintln!(
                        "job={} kind={} attempt={} code={}",
                        job.id, job.kind, job.attempt, error.code
                    );
                }
            });
        }
    }
}
pub async fn execute(app: &App, job: &Job) -> Result<()> {
    if job.kind == "material_translation" {
        return crate::reading::translate(app, job).await;
    }
    let records = match job.kind.as_str() {
        "formation" => sources::formation(app, job).await?,
        "transcribe" => audio::process(app, job).await?,
        "research" | "digest" => research::run(app, job).await?,
        "daily_summary" => crate::daily::run(app, job).await?,
        "cognition" => crate::cognition::run(app, job).await?,
        _ => return Err(Error::bad("任务类型无效")),
    };
    if !app.store.finish(job, &records, "处理完成")? {
        return Err(Error::missing());
    }
    Ok(())
}
fn mark_source_error(app: &App, job: &Job, error: &Error) {
    if !matches!(job.kind.as_str(), "transcribe" | "formation") {
        return;
    }
    let Some(key) = job.payload["sourceId"].as_str() else {
        return;
    };
    if let Ok(conn) = app.store.conn() {
        if job.kind == "transcribe" {
            let _=conn.execute("UPDATE records SET data=json_set(data,'$.status','failed','$.error',?) WHERE user_id=? AND kind='source' AND id=? AND EXISTS(SELECT 1 FROM jobs WHERE id=? AND user_id=? AND attempt=?)",params![error.message,job.user,key,job.id,job.user,job.attempt]);
        } else {
            let _=conn.execute("UPDATE records SET data=json_set(data,'$.analysisStatus',CASE WHEN (SELECT status FROM jobs WHERE id=?)='blocked' THEN 'blocked' WHEN (SELECT status FROM jobs WHERE id=?)='failed' THEN CASE WHEN COALESCE(json_extract(data,'$.analysisCoverage.scannedChars'),json_extract(data,'$.analysisCoverage.includedChars'),0)>0 THEN 'partial' ELSE 'failed' END ELSE 'pending' END,'$.analysisMessage',?) WHERE user_id=? AND kind='source' AND id=? AND EXISTS(SELECT 1 FROM jobs WHERE id=? AND user_id=? AND attempt=?)",params![job.id,job.id,error.message,job.user,key,job.id,job.user,job.attempt]);
        }
    }
}
pub fn schedule(app: &App) -> Result<()> {
    schedule_at(app, chrono::Utc::now())
}
pub fn schedule_at(app: &App, at: chrono::DateTime<chrono::Utc>) -> Result<()> {
    for user in app.store.users()? {
        let p = app.store.profile(&user)?;
        let tz = p["timezone"]
            .as_str()
            .unwrap_or("Asia/Shanghai")
            .parse::<chrono_tz::Tz>()
            .unwrap_or(chrono_tz::Asia::Shanghai);
        let local = at.with_timezone(&tz);
        let night = p["researchHour"].as_u64().unwrap_or(23) as u32;
        // Before tonight, catch up the most recent night after a restart.
        let date = if local.hour() >= night {
            local.date_naive()
        } else {
            local.date_naive().pred_opt().ok_or_else(Error::internal)?
        };
        let tomorrow = date.succ_opt().ok_or_else(Error::internal)?;
        let hour = p["digestHour"].as_u64().unwrap_or(8) as u32;
        let delivery = tz
            .from_local_datetime(
                &tomorrow
                    .and_hms_opt(hour, 0, 0)
                    .ok_or_else(Error::internal)?,
            )
            .earliest()
            .or_else(|| {
                tz.from_local_datetime(&tomorrow.and_hms_opt((hour + 1).min(23), 0, 0).unwrap())
                    .earliest()
            })
            .ok_or_else(Error::internal)?;
        let date = date.to_string();
        let available = delivery.with_timezone(&chrono::Utc).to_rfc3339();
        app.store.queue(
            &user,
            "daily_summary",
            json!({"localDate":date}),
            &format!("night-summary:{date}"),
        )?;
        let has_inferred_interest = app
            .store
            .list_all(&user, "cognition")?
            .iter()
            .any(|c| c["status"] == "active" && c["type"] == "interest");
        if !p["interests"].as_array().is_none_or(Vec::is_empty)
            || !p["feeds"].as_array().is_none_or(Vec::is_empty)
            || has_inferred_interest
        {
            app.store.queue(
                &user,
                "digest",
                json!({"kind":"digest","date":date,"researchDate":date,"availableAt":available}),
                &format!("digest:{date}"),
            )?;
        }
        let mut thoughts = app.store.list_all(&user, "thought")?;
        thoughts.reverse(); // New input enters the queue before untouched historical topics.
        for thought in thoughts {
            if !matches!(thought["status"].as_str(), Some("open" | "kept"))
                || thought["researchEligible"] == false
                || thought["mergedInto"].is_string()
            {
                continue;
            }
            let eligible_date = thought["createdAt"]
                .as_str()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|d| d.with_timezone(&tz).date_naive().to_string());
            if eligible_date.as_deref().is_none_or(|d| d > date.as_str()) {
                continue;
            }
            let key = thought["id"].as_str().ok_or_else(Error::internal)?;
            let revision = crate::hash(
                format!(
                    "{}:{}:{}:{}:{}",
                    thought["question"],
                    thought["gaps"],
                    thought["nextQuestion"],
                    thought["summary"],
                    thought["sourceIds"]
                )
                .as_bytes(),
            );
            app.store.queue(&user,"research",json!({"kind":"research","thoughtId":key,"date":date,"researchDate":date,"availableAt":available,"topicRevision":thought["revision"]}),&format!("research-content:{key}:{revision}"))?;
        }
    }
    Ok(())
}
pub fn retention(app: &App) -> Result<()> {
    crate::shared_public::retention(app)?;
    crate::agent::retention(app)?;
    let at = chrono::Utc::now().timestamp();
    let conn = app.store.conn()?;
    conn.execute("DELETE FROM records WHERE kind='material_translation' AND CAST(json_extract(data,'$.expiresAt') AS INTEGER)>0 AND CAST(json_extract(data,'$.expiresAt') AS INTEGER)<=?", [at])?;
    conn.execute("DELETE FROM records WHERE kind='raw' AND CAST(json_extract(data,'$.expiresAt') AS INTEGER)<?",[at])?;
    conn.execute(
        "UPDATE receipts SET response=NULL WHERE created<?",
        [at - 86400],
    )?;
    conn.execute("DELETE FROM receipts WHERE created<?", [at - 30 * 86400])?;
    conn.execute("DELETE FROM pairs WHERE expires<?", [at])?;
    conn.execute("UPDATE jobs SET status='failed',message='重试次数已耗尽',payload='{}' WHERE status='running' AND lease_until<? AND attempt>=3",[at])?;
    let expired: Vec<(String, String, String)> = {
        let mut stmt=conn.prepare("SELECT user_id,id,data FROM records WHERE kind='source' AND ((json_extract(data,'$.kind')='audio' AND json_extract(data,'$.status')!='ready' AND julianday(created)<julianday('now')-1) OR (json_extract(data,'$.status')='ready' AND julianday(created)<julianday('now')-COALESCE((SELECT json_extract(profile,'$.retentionDays') FROM users WHERE users.id=records.user_id),30))) LIMIT 100")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
    };
    drop(conn);
    for (user, key, _) in expired {
        match sources::delete_owned(app, &user, &key) {
            Ok(()) => {}
            Err(e) if e.code == "not_found" => {}
            Err(e) => return Err(e),
        }
    }
    // Orphans include a crash after fsync but before the input transaction.
    if let Ok(entries) = std::fs::read_dir(app.data_dir.join("audio")) {
        for entry in entries.flatten() {
            if entry
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age > Duration::from_secs(86400))
            {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    app.store
        .conn()?
        .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn busy_before_request_retries_and_terminal_analysis_failure_is_visible() {
        let dir = std::env::temp_dir().join(format!("memhot-busy-{}", crate::id()));
        let app = App::open(dir.clone()).unwrap();
        let (user, _) = crate::auth::create_pair(&app, "A", None).unwrap();
        let source = json!({"id":"source","kind":"note","text":"a question","status":"ready","createdAt":crate::now(),"analysisStatus":"pending"});
        sources::persist_input(&app, &user, &source, None, None, "formation").unwrap();
        for attempt in 1..=3 {
            let job = app.store.claim().unwrap().unwrap();
            assert_eq!(job.attempt, attempt);
            let error = Error::upstream("busy", "模型正在处理其他任务，请稍后重试");
            app.store.fail(&job, &error).unwrap();
            mark_source_error(&app, &job, &error);
            let state = app.store.get(&user, "source", "source").unwrap();
            assert_eq!(
                state["analysisStatus"],
                if attempt == 3 { "failed" } else { "pending" }
            );
            app.store
                .conn()
                .unwrap()
                .execute("UPDATE jobs SET next_at=0 WHERE id=?", [job.id])
                .unwrap();
        }
        assert_eq!(app.store.jobs(&user).unwrap()[0]["status"], "failed");
        let n: i64 = app
            .store
            .conn()
            .unwrap()
            .query_row("SELECT count(*) FROM receipts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0);
        let _ = std::fs::remove_dir_all(dir);
    }
}
