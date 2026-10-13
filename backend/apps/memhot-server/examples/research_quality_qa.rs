//! Explicit isolated paid end-to-end QA; never reads or publishes production user records.
use memhot_server::{billing, profile, research, App};
use serde_json::{json, Value};
use std::{os::unix::fs::PermissionsExt, path::PathBuf};
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = PathBuf::from(std::env::var("MEMHOT_RESEARCH_QA_DIR")?);
    let input = PathBuf::from(std::env::var("MEMHOT_RESEARCH_QA_INPUT")?);
    let resume = std::env::var("MEMHOT_RESEARCH_QA_RESUME").as_deref() == Ok("1");
    if !dir.is_absolute() || !dir.to_string_lossy().contains("/qa/") || (dir.exists() && !resume) {
        return Err("fresh isolated QA directory required; explicit resume only".into());
    }
    let case: Value = serde_json::from_slice(&std::fs::read(input)?)?;
    let app = App::open(dir.clone())?;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    let user = app
        .alphaxiv
        .as_ref()
        .as_ref()
        .map(|c| c.user_id.as_str())
        .unwrap_or("research-qa-only");
    let conn = app.store.conn()?;
    conn.execute(
        "INSERT OR IGNORE INTO users(id,name,profile) VALUES(?,'research quality QA',?)",
        rusqlite::params![user, profile::default_profile().to_string()],
    )?;
    billing::ensure_wallet(&conn, user)?;
    // Explicit QA budget; no real subscription or production balance is modified.
    conn.execute(
        "UPDATE billing_wallets SET included=30 WHERE user_id=? AND spent=0",
        [user],
    )?;
    let spent: i64 = conn.query_row(
        "SELECT spent FROM billing_wallets WHERE user_id=?",
        [user],
        |r| r.get(0),
    )?;
    if spent >= 30 {
        return Err("QA budget exhausted".into());
    }
    drop(conn);
    let mut thought = case;
    thought["id"] = json!("qa-thought");
    thought["analysisStatus"] = json!("ready");
    thought["status"] = json!("open");
    thought["createdAt"] = json!(memhot_server::now());
    app.store.put(user, "thought", &thought, None)?;
    app.store.queue(
        user,
        "research",
        json!({"kind":"research","thoughtId":"qa-thought"}),
        "quality-v3",
    )?;
    let conn = app.store.conn()?;
    let (job_id, attempt): (String, i64) = conn.query_row(
        "SELECT id,attempt FROM jobs WHERE user_id=? AND dedup='quality-v3'",
        [user],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if attempt >= 3 {
        return Err("bounded QA attempts exhausted".into());
    }
    conn.execute(
        "UPDATE jobs SET status='running',attempt=attempt+1,lease_until=? WHERE id=?",
        rusqlite::params![chrono::Utc::now().timestamp() + 1200, job_id],
    )?;
    drop(conn);
    let job = memhot_server::db::Job {
        id: job_id,
        user: user.into(),
        kind: "research".into(),
        payload: json!({"kind":"research","thoughtId":"qa-thought"}),
        attempt: attempt + 1,
    };
    let result = research::run(&app, &job).await;
    let outcome = match result {
        Ok(records) => {
            let report = records
                .iter()
                .find(|(k, _)| *k == "report")
                .ok_or("missing report")?
                .1
                .clone();
            // A research question may honestly remain unresolved. Acceptance requires
            // useful reviewed analysis, multiple grounded sources, and an honest status;
            // it must not pressure the model into pretending all gaps are resolved.
            let analysis = report["body"]
                .as_str()
                .is_some_and(|body| body.starts_with("## 如何推进这个想法"));
            let sources = report["sources"].as_array().cloned().unwrap_or_default();
            let grounded = sources.len() >= 2
                && sources
                    .iter()
                    .all(|s| s["verification"] == "quote_matched_and_support_reviewed");
            let complete = report["generator"]["quality"]["sufficient"] == true;
            let honest = report["status"]
                == if complete && analysis {
                    "ready"
                } else {
                    "partial"
                };
            let outcome = json!({"status":report["status"],"quality":report["generator"]["quality"],"claims":report["claims"].as_array().map(Vec::len),"sources":sources.len(),"hasReviewedAnalysis":analysis,"groundedSources":grounded,"honestStatus":honest,"passed":analysis && grounded && honest});
            std::fs::write(dir.join("report.json"), serde_json::to_vec_pretty(&report)?)?;
            std::fs::set_permissions(
                dir.join("report.json"),
                std::fs::Permissions::from_mode(0o600),
            )?;
            app.store.finish(&job, &records, "QA completed")?;
            outcome
        }
        Err(e) => {
            app.store.fail(&job, &e)?;
            json!({"passed":false,"code":e.code})
        }
    };
    std::fs::write(
        dir.join("outcome.json"),
        serde_json::to_vec_pretty(&outcome)?,
    )?;
    println!("{outcome}");
    if outcome["passed"] != true {
        return Err("quality QA not complete; inspect isolated result".into());
    }
    Ok(())
}
