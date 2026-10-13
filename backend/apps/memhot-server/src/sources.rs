use crate::{
    auth::User,
    clip,
    error::{Error, Result},
    hash, id, now, App,
};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use rusqlite::params;
use serde_json::{json, Value};

pub fn public(source: &Value) -> Value {
    let mut s = source.clone();
    if let Some(m) = s.as_object_mut() {
        m.retain(|k, _| !k.starts_with('_'));
    }
    s
}
pub use crate::import::user_text;
pub fn initial_thought(source: &Value) -> Option<Value> {
    let text = user_text(
        source["text"].as_str().unwrap_or_default(),
        source["kind"].as_str().unwrap_or("note"),
        source["speakerMode"].as_str().unwrap_or("roles"),
    );
    if text.trim().is_empty() {
        return None;
    }
    let quote = clip(text.trim(), 1200);
    Some(
        json!({"id":id(),"title":source["title"].as_str().filter(|s| !s.trim().is_empty() && *s!="日常想法").map(|s|clip(s,60)).unwrap_or_else(||clip(text.trim(),24)),"question":"这条想法还需要什么证据？","quote":quote,"sourceId":source["id"],"assumptions":[],"gaps":["尚待明确目标、具体问题与现实约束","尚待收集支持与相反证据"],"status":"open","createdAt":now(),"analysisStatus":"pending","researchEligible":false,"origin":"user_quote"}),
    )
}
pub async fn create(
    State(app): State<App>,
    User(user): User,
    Json(input): Json<Value>,
) -> Result<Json<Value>> {
    let text = input["text"]
        .as_str()
        .filter(|s| !s.trim().is_empty() && s.len() <= 200_000)
        .ok_or_else(|| Error::bad("请输入1至200000字节的文本"))?;
    let kind = input["kind"].as_str().unwrap_or("note");
    if !matches!(kind, "note" | "import") {
        return Err(Error::bad("来源类型无效"));
    }
    let title = input["title"].as_str().unwrap_or("日常想法");
    if title.len() > 600 {
        return Err(Error::bad("标题过长"));
    }
    let client = input["clientId"].as_str();
    if client.is_some_and(|s| s.len() > 200 || s.is_empty()) {
        return Err(Error::bad("clientId无效"));
    }
    if let Some(client) = client {
        if let Some(existing) = app.store.by_client(&user, "source", client)? {
            if existing["_hash"] != hash(text.as_bytes()) {
                return Err(Error::new(
                    StatusCode::CONFLICT,
                    "idempotency_conflict",
                    "相同clientId已用于不同内容",
                ));
            }
            return Ok(Json(public(&existing)));
        }
    }
    let speaker_mode = input["speakerMode"]
        .as_str()
        .unwrap_or(if kind == "import" { "roles" } else { "self" });
    if !matches!(speaker_mode, "roles" | "self" | "unknown") {
        return Err(Error::bad("speakerMode须为roles、self或unknown"));
    }
    let mut source = json!({"id":id(),"title":title,"kind":kind,"text":text,"createdAt":now(),"status":"ready","_hash":hash(text.as_bytes()),"speakerMode":speaker_mode});
    if let Some(at) = input["occurredAt"].as_str() {
        chrono::DateTime::parse_from_rfc3339(at).map_err(|_| Error::bad("来源时间无效"))?;
        source["occurredAt"] = json!(at);
    }
    let thought = if kind == "note" && speaker_mode != "unknown" {
        initial_thought(&source)
    } else {
        None
    };
    source["analysisStatus"] = json!(if user_text(text, kind, speaker_mode).is_empty() {
        "roles_required"
    } else {
        "pending"
    });
    if source["analysisStatus"] == "roles_required" {
        source["analysisMessage"]=json!("未找到明确的user角色。已保留原文；请选择本人自述导入，或使用带user/assistant角色的文本或JSON messages。");
    }
    if let Err(error) = persist_input(&app, &user, &source, thought.as_ref(), client, "formation") {
        if let Some(existing) = client
            .map(|client| app.store.by_client(&user, "source", client))
            .transpose()?
            .flatten()
        {
            if existing["_hash"] == source["_hash"] {
                return Ok(Json(public(&existing)));
            }
            return Err(Error::new(
                StatusCode::CONFLICT,
                "idempotency_conflict",
                "相同clientId已用于不同内容",
            ));
        }
        return Err(error);
    }
    Ok(Json(public(&source)))
}
pub fn persist_input(
    app: &App,
    user: &str,
    source: &Value,
    thought: Option<&Value>,
    client: Option<&str>,
    kind: &str,
) -> Result<()> {
    let mut conn = app.store.conn()?;
    let tx = conn.transaction()?;
    let source_id = source["id"].as_str().ok_or_else(Error::internal)?;
    tx.execute(
        "INSERT INTO records(user_id,kind,id,data,client_id,created) VALUES(?,'source',?,?,?,?)",
        params![
            user,
            source_id,
            source.to_string(),
            client,
            source["createdAt"].as_str()
        ],
    )?;
    if let Some(t) = thought {
        tx.execute(
            "INSERT INTO records(user_id,kind,id,data,created) VALUES(?,'thought',?,?,?)",
            params![
                user,
                t["id"].as_str(),
                t.to_string(),
                t["createdAt"].as_str()
            ],
        )?;
    }
    tx.execute("INSERT INTO jobs(id,user_id,kind,payload,status,created,dedup) VALUES(?,?,?,?,'queued',?,?)",params![id(),user,kind,json!({"sourceId":source_id,"thoughtId":thought.map(|t|&t["id"])}).to_string(),now(),format!("{kind}:{source_id}")])?;
    tx.commit()?;
    Ok(())
}
pub async fn get(
    State(app): State<App>,
    User(user): User,
    Path(key): Path<String>,
) -> Result<Json<Value>> {
    Ok(Json(public(&app.store.get(&user, "source", &key)?)))
}
pub async fn delete(
    State(app): State<App>,
    User(user): User,
    Path(key): Path<String>,
) -> Result<StatusCode> {
    delete_owned(&app, &user, &key)?;
    Ok(StatusCode::NO_CONTENT)
}
pub async fn analyze(
    State(app): State<App>,
    User(user): User,
    Path(key): Path<String>,
) -> Result<Json<Value>> {
    let mut conn = app.store.conn()?;
    let tx = conn.transaction()?;
    let mut source =
        crate::persistence::current(&tx, &user, "source", &key)?.ok_or_else(Error::missing)?;
    if source["text"].as_str().is_none_or(|s| s.trim().is_empty()) {
        return Err(Error::bad("暂无可分析的文字"));
    }
    use rusqlite::OptionalExtension;
    let existing:Option<(String,String)>=tx.query_row("SELECT id,status FROM jobs WHERE user_id=? AND kind='formation' AND json_extract(payload,'$.sourceId')=? AND status IN ('queued','running','blocked') ORDER BY created DESC LIMIT 1",params![user,key],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    if let Some((id, status)) = existing {
        return Ok(Json(json!({"id":id,"status":status})));
    }
    // Completed windows remain reusable after retries. Explicit reanalysis re-extracts.
    tx.execute("DELETE FROM records WHERE user_id=? AND kind='formation_window' AND json_extract(data,'$.sourceId')=?",params![user,key])?;
    source["analysisStatus"] = json!("pending");
    source["analysisMessage"] = Value::Null;
    source["analysisIssues"] = json!([]);
    let total = user_text(
        source["text"].as_str().unwrap_or_default(),
        source["kind"].as_str().unwrap_or("note"),
        source["speakerMode"].as_str().unwrap_or("roles"),
    )
    .chars()
    .count();
    source["analysisCoverage"] = json!({"includedChars":0,"scannedChars":0,"totalChars":total,"complete":false,"completedWindows":0,"rejectedItems":0});
    crate::persistence::put(&tx, &user, "source", &source)?;
    let job = id();
    tx.execute("INSERT INTO jobs(id,user_id,kind,payload,status,created,dedup) VALUES(?,?,'formation',?,'queued',?,?)",params![job,user,json!({"sourceId":key}).to_string(),now(),format!("reanalyze:{job}")])?;
    tx.commit()?;
    Ok(Json(json!({"id":job,"status":"queued"})))
}
#[path = "source_cleanup.rs"]
mod cleanup;
pub fn delete_owned(app: &App, user: &str, key: &str) -> Result<()> {
    cleanup::delete_owned(app, user, key)
}
pub use crate::formation::run as formation;
pub async fn feedback(
    State(app): State<App>,
    User(user): User,
    Path(key): Path<String>,
    Json(input): Json<Value>,
) -> Result<Json<Value>> {
    let status = match input["action"].as_str() {
        Some("keep") => "kept",
        Some("dismiss") => "dismissed",
        Some("resolved") => "resolved",
        _ => return Err(Error::bad("反馈操作无效")),
    };
    let note = input["note"].as_str();
    if note.is_some_and(|s| s.len() > 6000) {
        return Err(Error::bad("反馈内容过长"));
    }
    let mut conn = app.store.conn()?;
    let tx = conn.transaction()?;
    let changed=tx.execute("UPDATE records SET data=json_set(data,'$.status',?,'$.researchEligible',json(?),'$.feedbackNote',COALESCE(?,json_extract(data,'$.feedbackNote')),'$.revision',COALESCE(json_extract(data,'$.revision'),0)+1) WHERE user_id=? AND kind='thought' AND id=?",params![status,if status=="kept"{"true"}else{"false"},note,user,key])?;
    if changed == 0 {
        return Err(Error::missing());
    }
    if let Some(note) = note.filter(|s| !s.trim().is_empty()) {
        crate::cognition::enqueue_observation(
            &tx,
            &user,
            &json!({"id":id(),"thoughtId":key,"text":note,"createdAt":now()}),
        )?;
    }
    crate::daily::enqueue_today(&tx, &user)?;
    tx.commit()?;
    drop(conn);
    Ok(Json(app.store.get(&user, "thought", &key)?))
}
