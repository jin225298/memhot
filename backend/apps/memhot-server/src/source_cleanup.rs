//! Remove one source without erasing independent evidence from other recordings.
use crate::{
    cognition, daily,
    error::{Error, Result},
    id, now, persistence, App,
};
use rusqlite::{params, Transaction};
use serde_json::{json, Value};
use std::collections::HashSet;

fn list(tx: &Transaction<'_>, user: &str, kind: &str) -> Result<Vec<Value>> {
    let mut stmt = tx.prepare("SELECT data FROM records WHERE user_id=? AND kind=?")?;
    let rows = stmt.query_map(params![user, kind], |r| r.get::<_, String>(0))?;
    rows.map(|r| Ok(serde_json::from_str(&r?)?)).collect()
}
fn remove(tx: &Transaction<'_>, user: &str, kind: &str, key: &str) -> Result<()> {
    tx.execute(
        "DELETE FROM records WHERE user_id=? AND kind=? AND id=?",
        params![user, kind, key],
    )?;
    Ok(())
}
fn retained_topic(mut thought: Value, source: &str) -> Option<Value> {
    let edited = thought["editedFields"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let independent = thought["origin"] == "manual" || !edited.is_empty();
    let mut remaining = thought["sourceIds"].as_array().cloned().unwrap_or_default();
    if thought["sourceId"].is_string() && !remaining.contains(&thought["sourceId"]) {
        remaining.push(thought["sourceId"].clone());
    }
    remaining.retain(|s| s != source);
    if remaining.is_empty() && !independent {
        return None;
    }
    thought["sourceIds"] = json!(remaining);
    if thought["sourceId"] == source {
        thought["sourceId"] = remaining.first().cloned().unwrap_or(Value::Null);
    }
    let refs: Vec<_> = thought["evidenceRefs"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|e| e["sourceId"] != source)
        .map(|e| {
            let mut e = e.clone();
            e.as_object_mut().map(|e| e.remove("summary"));
            e
        })
        .collect();
    thought["evidenceRefs"] = json!(refs);
    let manual = thought["origin"] == "manual";
    for field in [
        "title",
        "question",
        "summary",
        "assumptions",
        "gaps",
        "quote",
        "latestQuote",
        "nextQuestion",
        "lastDiscussionAt",
        "lastResearchedAt",
    ] {
        if edited.contains(&json!(field))
            || (manual && matches!(field, "title" | "question" | "summary"))
        {
            continue;
        }
        thought.as_object_mut().map(|v| v.remove(field));
    }
    if thought["title"].is_null() {
        thought["title"] = json!("等待按保留的来源重新整理");
    }
    if !edited.contains(&json!("question")) && !manual {
        thought["fingerprint"] = Value::Null;
        thought["fingerprints"] = json!([]);
    }
    if remaining.is_empty() && independent {
        thought["origin"] = json!("manual");
    }
    thought["analysisStatus"] = json!(if remaining.is_empty() {
        "ready"
    } else {
        "pending"
    });
    thought["researchEligible"] =
        json!(independent && !matches!(thought["status"].as_str(), Some("dismissed" | "resolved")));
    thought["revision"] = json!(thought["revision"].as_u64().unwrap_or(0) + 1);
    thought["updatedAt"] = json!(now());
    Some(thought)
}
fn sanitize_report(report: &Value) -> Value {
    let mut clean = json!({"personalContextDeleted":true,"thoughtId":null,"title":"个人记录已删除 · 公开研究材料",
        "summary":"原始个人记录与派生讨论已删除，仅保留公开来源供核对。","body":"原始个人记录已删除。下方公开来源仍可核对；本报告不再参与该思考的个性化延续。",
        "claims":[],"gaps":[],"status":"partial"});
    // Explicit allowlist prevents question/future private reasoning fields surviving deletion.
    for field in [
        "id",
        "kind",
        "createdAt",
        "availableAt",
        "researchDate",
        "localDate",
    ] {
        if let Some(value) = report.get(field) {
            clean[field] = value.clone();
        }
    }
    clean["sources"] = json!(report["sources"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|source| {
            let mut public = json!({"stance":"context"});
            // citations.claim can contain a personal inference even though the quoted page is public.
            for field in [
                "id",
                "title",
                "url",
                "publisher",
                "publishedAt",
                "retrievedAt",
                "excerpt",
                "retrievalKind",
                "publicationDateSource",
            ] {
                if let Some(value) = source.get(field) {
                    public[field] = value.clone();
                }
            }
            public
        })
        .collect::<Vec<_>>());
    clean
}
fn restart_source(tx: &Transaction<'_>, user: &str, key: &str) -> Result<()> {
    let Some(mut source) = persistence::current(tx, user, "source", key)? else {
        return Ok(());
    };
    tx.execute("DELETE FROM jobs WHERE user_id=? AND kind='formation' AND json_extract(payload,'$.sourceId')=?",params![user,key])?;
    tx.execute("DELETE FROM records WHERE user_id=? AND kind='formation_window' AND json_extract(data,'$.sourceId')=?",params![user,key])?;
    source["analysisStatus"] = json!("pending");
    source["analysisMessage"] = json!("已撤销删除来源的派生内容，正在按保留来源重新整理");
    source["analysisCoverage"] = json!({"includedChars":0,"totalChars":source["text"].as_str().unwrap_or_default().chars().count(),"complete":false});
    persistence::put(tx, user, "source", &source)?;
    let job = id();
    tx.execute("INSERT INTO jobs(id,user_id,kind,payload,status,created,dedup) VALUES(?,?,'formation',?,'queued',?,?)",params![job,user,json!({"sourceId":key}).to_string(),now(),format!("source-pruned:{job}")])?;
    daily::rebuild_source_date(tx, user, &source)
}
pub(super) fn delete_owned(app: &App, user: &str, key: &str) -> Result<()> {
    let mut conn = app.store.conn()?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let source = persistence::current(&tx, user, "source", key)?.ok_or_else(Error::missing)?;
    let thoughts: Vec<_> = list(&tx, user, "thought")?
        .into_iter()
        .filter(|t| {
            t["sourceId"] == key
                || t["sourceIds"]
                    .as_array()
                    .is_some_and(|a| a.contains(&json!(key)))
        })
        .collect();
    let thought_ids: Vec<_> = thoughts
        .iter()
        .filter_map(|t| t["id"].as_str().map(str::to_owned))
        .collect();
    let mut remaining = HashSet::new();
    for thought in thoughts {
        let thought_id = thought["id"]
            .as_str()
            .ok_or_else(Error::internal)?
            .to_owned();
        if let Some(retained) = retained_topic(thought.clone(), key) {
            remaining.extend(
                retained["sourceIds"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|s| s.as_str().map(str::to_owned)),
            );
            persistence::put(&tx, user, "thought", &retained)?;
        } else {
            persistence::tombstone(&tx, user, "thought", &thought)?;
            remove(&tx, user, "thought", &thought_id)?;
        }
        tx.execute(
            "DELETE FROM jobs WHERE user_id=? AND json_extract(payload,'$.thoughtId')=?",
            params![user, thought_id],
        )?;
    }
    let mut report_ids = HashSet::new();
    for report in list(&tx, user, "report")? {
        if report["thoughtId"]
            .as_str()
            .is_some_and(|id| thought_ids.iter().any(|t| t == id))
        {
            report_ids.insert(
                report["id"]
                    .as_str()
                    .ok_or_else(Error::internal)?
                    .to_owned(),
            );
            persistence::put(&tx, user, "report", &sanitize_report(&report))?;
        }
    }
    let mut conversations = Vec::new();
    for conversation in list(&tx, user, "conversation")? {
        if conversation["thoughtId"]
            .as_str()
            .is_some_and(|id| thought_ids.iter().any(|t| t == id))
            || conversation["reportId"]
                .as_str()
                .is_some_and(|id| report_ids.contains(id))
        {
            let id = conversation["id"].as_str().ok_or_else(Error::internal)?;
            conversations.push(id.to_owned());
            remove(&tx, user, "conversation", id)?;
        }
    }
    cognition::revoke_context(&tx, user, Some(key), &thought_ids, &conversations)?;
    tx.execute("DELETE FROM records WHERE user_id=? AND (kind='source' AND id=? OR kind='formation_window' AND json_extract(data,'$.sourceId')=? OR kind='daily_summary' AND EXISTS(SELECT 1 FROM json_each(data,'$.sourceIds') WHERE value=?))",params![user,key,key,key])?;
    tx.execute(
        "DELETE FROM jobs WHERE user_id=? AND json_extract(payload,'$.sourceId')=?",
        params![user, key],
    )?;
    for source in remaining {
        restart_source(&tx, user, &source)?;
    }
    if !list(&tx, user, "source")?.is_empty() || !list(&tx, user, "thought")?.is_empty() {
        daily::rebuild_source_date(&tx, user, &source)?;
    }
    tx.execute("DELETE FROM receipts WHERE user_id=? AND job_id NOT IN(SELECT id FROM jobs WHERE user_id=?)",params![user,user])?;
    crate::persistence::revoke_agent_context(&tx, user)?;
    tx.commit()?;
    drop(conn);
    if let Some(file) = source["_audioPath"].as_str() {
        let _ = std::fs::remove_file(app.data_dir.join("audio").join(file));
    }
    Ok(())
}
