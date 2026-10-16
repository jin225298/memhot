//! Fenced, atomic background commits. Human edits and deletion always win.
use crate::{
    db::Job,
    error::{Error, Result},
    now,
};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde_json::{json, Value};

pub(crate) fn current(
    tx: &Transaction<'_>,
    user: &str,
    kind: &str,
    key: &str,
) -> Result<Option<Value>> {
    let raw: Option<String> = tx
        .query_row(
            "SELECT data FROM records WHERE user_id=? AND kind=? AND id=?",
            params![user, kind, key],
            |r| r.get(0),
        )
        .optional()?;
    raw.map(|s| serde_json::from_str(&s).map_err(Into::into))
        .transpose()
}
pub(crate) fn put(tx: &Transaction<'_>, user: &str, kind: &str, value: &Value) -> Result<()> {
    tx.execute("INSERT INTO records(user_id,kind,id,data,created) VALUES(?,?,?,?,?) ON CONFLICT(user_id,kind,id) DO UPDATE SET data=excluded.data",
        params![user,kind,value["id"].as_str(),value.to_string(),value["createdAt"].as_str().unwrap_or("")])?;
    if kind == "thought" {
        crate::thought_days::initialize(tx, user, value)?;
    }
    Ok(())
}
/// Deleting private context revokes in-flight work and its retained continuation data.
pub(crate) fn revoke_agent_context(tx: &Transaction<'_>, user: &str) -> Result<()> {
    tx.execute(
        "DELETE FROM records WHERE user_id=? AND kind IN ('discussion_request','agent_run')",
        [user],
    )?;
    tx.execute("UPDATE users SET epoch=epoch+1 WHERE id=?", [user])?;
    tx.execute("UPDATE receipts SET response=NULL WHERE user_id=?", [user])?;
    Ok(())
}
pub(crate) fn tombstoned(tx: &Transaction<'_>, user: &str, kind: &str, key: &str) -> Result<bool> {
    Ok(current(tx, user, "tombstone", &format!("{kind}:{key}"))?.is_some())
}
pub(crate) fn tombstone(tx: &Transaction<'_>, user: &str, kind: &str, value: &Value) -> Result<()> {
    let key = value["id"].as_str().ok_or_else(Error::internal)?;
    put(
        tx,
        user,
        "tombstone",
        &json!({"id":format!("{kind}:{key}"),"kind":kind,"targetId":key,"fingerprint":value["fingerprint"],"fingerprints":value["fingerprints"],"evidenceIds":value["evidenceRefs"].as_array().into_iter().flatten().filter_map(|e|e["id"].as_str()).collect::<Vec<_>>(),"createdAt":now()}),
    )
}
fn source_exists(tx: &Transaction<'_>, user: &str, key: &str) -> Result<bool> {
    Ok(current(tx, user, "source", key)?.is_some())
}
fn union(owned: &mut Value, incoming: &Value, field: &str) {
    let mut values = owned[field].as_array().cloned().unwrap_or_default();
    for value in incoming[field].as_array().into_iter().flatten() {
        if !values.contains(value) {
            values.push(value.clone());
        }
    }
    owned[field] = json!(values);
}
fn merge_thought(mut owned: Value, value: &Value) -> Value {
    let edited = owned["editedFields"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    for field in [
        "title",
        "question",
        "summary",
        "assumptions",
        "gaps",
        "analysisStatus",
        "updatedAt",
        "latestQuote",
        "researchEligible",
    ] {
        if !edited.contains(&json!(field)) {
            if let Some(v) = value.get(field) {
                owned[field] = v.clone();
            }
        }
    }
    union(&mut owned, value, "sourceIds");
    union(&mut owned, value, "evidenceRefs");
    union(&mut owned, value, "fingerprints");
    if !edited.contains(&json!("question")) {
        if let Some(fingerprint) = value.get("fingerprint") {
            owned["fingerprint"] = fingerprint.clone();
        }
    }
    if owned["status"] == "kept" {
        owned["researchEligible"] = json!(true);
    }
    if matches!(owned["status"].as_str(), Some("dismissed" | "resolved")) {
        owned["researchEligible"] = json!(false);
    }
    owned["revision"] = json!(owned["revision"].as_u64().unwrap_or(0) + 1);
    owned
}
fn merge_public_reference(value: &mut Value, owned: &Value) {
    if value["publicEditionId"].as_str().is_none()
        || value["publicEditionId"] != owned["publicEditionId"]
    {
        return;
    }
    value["createdAt"] = owned["createdAt"].clone();
    let date = |v: &Value| {
        v["availableAt"]
            .as_str()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
    };
    match (date(owned), date(value)) {
        (Some(a), Some(b)) => {
            if a < b {
                value["availableAt"] = owned["availableAt"].clone();
            }
        }
        // Once immediately available, a later scheduled writer cannot hide the same edition.
        _ => {
            value.as_object_mut().map(|v| v.remove("availableAt"));
        }
    }
}
pub fn finish(
    conn: &mut Connection,
    job: &Job,
    records: &[(&str, Value)],
    message: &str,
) -> Result<bool> {
    commit(conn, job, records, Some(message))
}
pub fn checkpoint(conn: &mut Connection, job: &Job, records: &[(&str, Value)]) -> Result<bool> {
    commit(conn, job, records, None)
}
fn commit(
    conn: &mut Connection,
    job: &Job,
    records: &[(&str, Value)],
    message: Option<&str>,
) -> Result<bool> {
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let active = crate::research::job_active(&tx, job)?;
    if !active {
        return Ok(false);
    }
    let immediate: bool = tx.query_row("SELECT COALESCE(json_extract(payload,'$.deliverImmediately'),0) FROM jobs WHERE id=? AND user_id=?", params![job.id,job.user], |r| r.get(0))?;
    // A source may be deleted after the model finishes but before publication.
    // Check its captured account epoch in this same transaction as all outputs.
    let epoch: i64 = tx.query_row("SELECT epoch FROM users WHERE id=?", [&job.user], |row| {
        row.get(0)
    })?;
    if records.iter().any(|(_, value)| {
        value
            .get("_userEpoch")
            .is_some_and(|expected| expected.as_i64() != Some(epoch))
    }) {
        return Ok(false);
    }
    let mut outputs: Vec<(&str, Value)> = Vec::new();
    for (kind, value) in records {
        let key = value["id"].as_str().ok_or_else(Error::internal)?;
        let mut value = value.clone();
        if matches!(*kind, "thought" | "cognition") {
            if tombstoned(&tx, &job.user, kind, key)? {
                if value["_mustExist"] == true {
                    return Ok(false);
                }
                continue;
            }
            if let Some(fingerprint) = value["fingerprint"].as_str() {
                let deleted:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM records WHERE user_id=? AND kind='tombstone' AND json_extract(data,'$.kind')=? AND (json_extract(data,'$.fingerprint')=?3 OR EXISTS(SELECT 1 FROM json_each(data,'$.fingerprints') WHERE value=?3)))",params![job.user,kind,fingerprint],|r|r.get(0))?;
                if deleted {
                    continue;
                }
            }
        }
        match *kind {
            "source" => {
                let Some(mut owned) = current(&tx, &job.user, "source", key)? else {
                    return Ok(false);
                };
                if let Some(expected) = value.get("_analysisHash") {
                    if &owned["_hash"] != expected {
                        return Ok(false);
                    }
                }
                // Sources are immutable inputs; background jobs only modify derived state.
                for field in [
                    "text",
                    "status",
                    "error",
                    "analysisStatus",
                    "analysisMessage",
                    "analysisCoverage",
                    "analysisIssues",
                    "topicIds",
                    "_hash",
                    "_audioPath",
                ] {
                    if let Some(v) = value.get(field) {
                        owned[field] = v.clone();
                    }
                }
                value = owned;
            }
            "thought" => {
                let mut sources = value["sourceIds"].as_array().cloned().unwrap_or_default();
                if let Some(source) = value["sourceId"].as_str() {
                    sources.push(json!(source));
                }
                for source in sources.iter().filter_map(Value::as_str) {
                    if !source_exists(&tx, &job.user, source)? {
                        return Ok(false);
                    }
                }
                let owned = outputs
                    .iter()
                    .rev()
                    .find(|(kind, v)| *kind == "thought" && v["id"] == key)
                    .map(|(_, v)| v.clone())
                    .or(current(&tx, &job.user, "thought", key)?);
                if value["_mustExist"] == true && owned.is_none() {
                    return Ok(false);
                }
                if let Some(owned) = owned {
                    value = merge_thought(owned, &value);
                }
            }
            "formation_window" => {
                let source = value["sourceId"].as_str().ok_or_else(Error::internal)?;
                if !source_exists(&tx, &job.user, source)? {
                    return Ok(false);
                }
            }
            "cognition" => {
                for evidence in value["evidenceRefs"].as_array().into_iter().flatten() {
                    for (field, kind) in [
                        ("sourceId", "source"),
                        ("conversationId", "conversation"),
                        ("thoughtId", "thought"),
                    ] {
                        if let Some(id) = evidence[field].as_str() {
                            if current(&tx, &job.user, kind, id)?.is_none() {
                                return Ok(false);
                            }
                        }
                    }
                }
                let known:Option<String>=tx.query_row("SELECT data FROM records WHERE user_id=? AND kind='cognition' AND (id=? OR json_extract(data,'$.fingerprint')=?) LIMIT 1",params![job.user,key,value["fingerprint"].as_str()],|r|r.get(0)).optional()?;
                if let Some(mut owned) = known
                    .map(|raw| serde_json::from_str::<Value>(&raw))
                    .transpose()?
                {
                    if owned["origin"] == "manual"
                        || owned["userEdited"] == true
                        || owned["status"] == "dismissed"
                    {
                        continue;
                    }
                    value["id"] = owned["id"].clone();
                    union(&mut owned, &value, "evidenceRefs");
                    value["evidenceRefs"] = owned["evidenceRefs"].clone();
                    value["revision"] = json!(owned["revision"].as_u64().unwrap_or(0) + 1);
                }
            }
            "daily_summary" => {
                for source in value["sourceIds"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                {
                    if !source_exists(&tx, &job.user, source)? {
                        return Ok(false);
                    }
                }
            }
            "report" => {
                if current(&tx, &job.user, "report_tombstone", key)?.is_some() {
                    continue;
                }
                if let Some(thought) = value["thoughtId"].as_str() {
                    if !crate::research::thought_active(&tx, &job.user, thought)? {
                        return Ok(false);
                    }
                }
                if immediate {
                    value.as_object_mut().map(|v| v.remove("availableAt"));
                }
                if let Some(owned) = current(&tx, &job.user, "report", key)? {
                    merge_public_reference(&mut value, &owned);
                }
            }
            "merge_seed" => {
                if let Some(mut owned) = current(&tx, &job.user, "thought", key)? {
                    if owned["status"] == "open"
                        && owned["editedFields"].as_array().is_none_or(Vec::is_empty)
                    {
                        tx.execute(
                            "DELETE FROM records WHERE user_id=? AND kind='thought' AND id=?",
                            params![job.user, key],
                        )?;
                    } else {
                        owned["mergedInto"] = value["mergedInto"].clone();
                        owned["researchEligible"] = json!(false);
                        put(&tx, &job.user, "thought", &owned)?;
                    }
                }
                continue;
            }
            _ => {}
        }
        if let Some(map) = value.as_object_mut() {
            map.remove("_mustExist");
            map.remove("_analysisHash");
            map.remove("_userEpoch");
        }
        outputs.push((*kind, value));
    }
    for (kind, value) in &outputs {
        put(&tx, &job.user, kind, value)?;
        if *kind == "report" {
            if let Some(thought) = value["thoughtId"].as_str() {
                tx.execute("UPDATE records SET data=json_set(data,'$.lastResearchedAt',?) WHERE user_id=? AND kind='thought' AND id=?",params![now(),job.user,thought])?;
            }
        }
    }
    if let Some(message) = message {
        if job.kind == "transcribe" {
            if let Some((_, source)) = outputs.iter().find(|(kind, source)| {
                *kind == "source"
                    && source["text"]
                        .as_str()
                        .is_some_and(|s| !s.trim().is_empty())
            }) {
                tx.execute("INSERT OR IGNORE INTO jobs(id,user_id,kind,payload,status,created,dedup) VALUES(?,?,'formation',?,'queued',?,?)",
                    params![crate::id(),job.user,json!({"sourceId":source["id"]}).to_string(),now(),format!("formation:{}",source["id"].as_str().unwrap_or_default())])?;
            }
        }
        if job.kind == "formation" {
            if let Some(source) = job.payload["sourceId"]
                .as_str()
                .and_then(|id| current(&tx, &job.user, "source", id).ok().flatten())
            {
                crate::daily::enqueue_source(&tx, &job.user, &source)?;
            }
        }
        tx.execute("UPDATE jobs SET status='completed',payload='{}',message=?,lease_until=0 WHERE id=? AND user_id=? AND attempt=?",params![message,job.id,job.user,job.attempt])?;
        tx.execute(
            "UPDATE receipts SET response=NULL WHERE user_id=? AND job_id=?",
            params![job.user, job.id],
        )?;
    }
    tx.commit()?;
    Ok(true)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn late_public_delivery_never_hides_an_already_visible_reference() {
        let immediate = json!({"id":"report","publicEditionId":"edition","createdAt":"first"});
        let scheduled = json!({"id":"report","publicEditionId":"edition","createdAt":"later","availableAt":"2026-10-12T08:00:00Z"});
        let mut late = scheduled.clone();
        merge_public_reference(&mut late, &immediate);
        assert!(late.get("availableAt").is_none());
        assert_eq!(late["createdAt"], "first");
        let mut manual = immediate;
        merge_public_reference(&mut manual, &scheduled);
        assert!(manual.get("availableAt").is_none());
        let earlier =
            json!({"publicEditionId":"edition","availableAt":"2026-10-12T15:00:00+08:00"});
        let mut later = scheduled;
        merge_public_reference(&mut later, &earlier);
        assert_eq!(later["availableAt"], earlier["availableAt"]);
    }
}
