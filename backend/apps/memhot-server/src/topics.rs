//! User-owned topic editing and deletion; model jobs may never undo these changes.
use crate::{
    auth::User,
    error::{Error, Result},
    id, now, persistence, App,
};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use rusqlite::params;
use serde::Deserialize;
use serde_json::{json, Value};
#[path = "thought_cognition.rs"]
mod memories;

pub fn fingerprint(text: &str) -> String {
    crate::hash(
        text.chars()
            .filter(|c| c.is_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect::<String>()
            .as_bytes(),
    )
}
pub fn conflict() -> Error {
    Error::new(
        StatusCode::CONFLICT,
        "revision_conflict",
        "内容已被更新，请刷新后重试",
    )
}
pub fn check_revision(current: &Value, input: &Value) -> Result<()> {
    if let Some(revision) = input["revision"].as_u64() {
        if revision != current["revision"].as_u64().unwrap_or(1) {
            return Err(conflict());
        }
    }
    Ok(())
}
pub fn edit(value: &mut Value, input: &Value) -> Result<()> {
    let mut fingerprints = value["fingerprints"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    if value["fingerprint"].is_string() && !fingerprints.contains(&value["fingerprint"]) {
        fingerprints.push(value["fingerprint"].clone());
    }
    let map = input
        .as_object()
        .ok_or_else(|| Error::bad("话题必须是JSON对象"))?;
    let mut edited = value["editedFields"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    for (key, v) in map {
        match key.as_str() {
            "title" | "question" | "summary" => {
                let text = v
                    .as_str()
                    .filter(|s| {
                        (key == "summary" || !s.trim().is_empty()) && s.chars().count() <= 3000
                    })
                    .ok_or_else(|| Error::bad("话题内容须为1至3000字"))?;
                value[key] = json!(text.trim());
                if !edited.contains(&json!(key)) {
                    edited.push(json!(key));
                }
            }
            "category" => {
                let text = v
                    .as_str()
                    .filter(|s| s.trim().chars().count() <= 40)
                    .ok_or_else(|| Error::bad("分类须为不超过40字的文字"))?;
                value[key] = json!(if text.trim().is_empty() {
                    "未分类"
                } else {
                    text.trim()
                });
                if !edited.contains(&json!(key)) {
                    edited.push(json!(key));
                }
            }
            "status" => {
                if !matches!(v.as_str(), Some("open" | "kept" | "dismissed" | "resolved")) {
                    return Err(Error::bad("话题状态无效"));
                }
                value[key] = v.clone();
            }
            "sourceIds" => {
                let ids = v
                    .as_array()
                    .filter(|a| a.len() <= 128 && a.iter().all(Value::is_string))
                    .ok_or_else(|| Error::bad("来源列表无效"))?;
                value[key] = json!(ids);
            }
            "revision" => {}
            _ => return Err(Error::bad("未知话题字段")),
        }
    }
    value["editedFields"] = json!(edited);
    value["researchEligible"] = json!(!matches!(
        value["status"].as_str(),
        Some("dismissed" | "resolved")
    ));
    value["revision"] = json!(value["revision"].as_u64().unwrap_or(0) + 1);
    value["updatedAt"] = json!(now());
    value["fingerprint"] = json!(fingerprint(value["question"].as_str().unwrap_or_default()));
    if !fingerprints.contains(&value["fingerprint"]) {
        fingerprints.push(value["fingerprint"].clone());
    }
    value["fingerprints"] = json!(fingerprints);
    Ok(())
}
fn validate_sources(tx: &rusqlite::Transaction<'_>, user: &str, value: &Value) -> Result<()> {
    for key in value["sourceIds"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        if persistence::current(tx, user, "source", key)?.is_none() {
            return Err(Error::missing());
        }
    }
    Ok(())
}
pub async fn create(
    State(app): State<App>,
    User(user): User,
    Json(input): Json<Value>,
) -> Result<Json<Value>> {
    let question = input["question"]
        .as_str()
        .or_else(|| input["title"].as_str())
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| Error::bad("请填写话题或问题"))?;
    let mut value = json!({"id":id(),"title":crate::clip(question,80),"question":question,"summary":question,"sourceIds":[],"evidenceRefs":[],"status":"open","origin":"manual","category":"未分类","analysisStatus":"ready","createdAt":now()});
    edit(&mut value, &input)?;
    let mut conn = app.store.conn()?;
    let tx = conn.transaction()?;
    validate_sources(&tx, &user, &value)?;
    persistence::put(&tx, &user, "thought", &value)?;
    crate::research::cancel_archived(&tx, &user, &value)?;
    memories::manual_interest(&tx, &user, &value, Some(&input))?;
    crate::daily::enqueue_today(&tx, &user)?;
    crate::thought_days::annotate(&tx, &user, &mut value, None)?;
    tx.commit()?;
    Ok(Json(value))
}
pub async fn update(
    State(app): State<App>,
    User(user): User,
    Path(key): Path<String>,
    Json(input): Json<Value>,
) -> Result<Json<Value>> {
    let mut conn = app.store.conn()?;
    let tx = conn.transaction()?;
    let mut value =
        persistence::current(&tx, &user, "thought", &key)?.ok_or_else(Error::missing)?;
    if value["status"] == "deleted" {
        return Err(Error::missing());
    }
    check_revision(&value, &input)?;
    let explicit_content: serde_json::Map<String, Value> = ["title", "question", "summary"]
        .into_iter()
        .filter_map(|field| {
            input[field]
                .as_str()
                .filter(|text| value[field].as_str().unwrap_or_default() != text.trim())
                .map(|text| (field.to_owned(), json!(text.trim())))
        })
        .collect();
    edit(&mut value, &input)?;
    validate_sources(&tx, &user, &value)?;
    persistence::put(&tx, &user, "thought", &value)?;
    crate::research::cancel_archived(&tx, &user, &value)?;
    memories::manual_interest(
        &tx,
        &user,
        &value,
        (!explicit_content.is_empty()).then_some(&json!(explicit_content)),
    )?;
    crate::daily::enqueue_today(&tx, &user)?;
    crate::thought_days::annotate(&tx, &user, &mut value, None)?;
    tx.commit()?;
    Ok(Json(value))
}
#[derive(Deserialize)]
pub struct DeleteOptions {
    preference: Option<String>,
    revision: Option<u64>,
}
pub async fn delete(
    State(app): State<App>,
    User(user): User,
    Path(key): Path<String>,
    Query(options): Query<DeleteOptions>,
) -> Result<StatusCode> {
    if !matches!(options.preference.as_deref(), None | Some("none" | "less")) {
        return Err(Error::bad("删除偏好无效"));
    }
    let mut conn = app.store.conn()?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let Some(value) = persistence::current(&tx, &user, "thought", &key)? else {
        return if persistence::tombstoned(&tx, &user, "thought", &key)? {
            Ok(StatusCode::NO_CONTENT)
        } else {
            Err(Error::missing())
        };
    };
    check_revision(&value, &json!({"revision":options.revision}))?;
    if options.preference.as_deref() == Some("less") {
        memories::deletion_preference(&tx, &user, &value)?;
    }
    persistence::tombstone(&tx, &user, "thought", &value)?;
    let conversations: Vec<String> = {
        let mut stmt=tx.prepare("SELECT id FROM records WHERE user_id=? AND kind='conversation' AND (json_extract(data,'$.thoughtId')=? OR json_extract(data,'$.reportId') IN(SELECT id FROM records WHERE user_id=? AND kind='report' AND json_extract(data,'$.thoughtId')=?))")?;
        let rows = stmt.query_map(params![user, key, user, key], |r| r.get(0))?;
        rows.collect::<std::result::Result<_, _>>()?
    };
    crate::cognition::revoke_context(&tx, &user, None, &[key.clone()], &conversations)?;
    for conversation in conversations {
        tx.execute(
            "DELETE FROM records WHERE user_id=? AND kind='conversation' AND id=?",
            params![user, conversation],
        )?;
    }
    tx.execute("DELETE FROM records WHERE user_id=? AND (kind='thought' AND id=? OR kind IN ('report','conversation') AND json_extract(data,'$.thoughtId')=?)",params![user,key,key])?;
    tx.execute(
        "DELETE FROM jobs WHERE user_id=? AND json_extract(payload,'$.thoughtId')=?",
        params![user, key],
    )?;
    // Rebuild summaries rather than leaving removed topics in materialized sections.
    tx.execute("DELETE FROM records WHERE user_id=? AND kind='daily_summary' AND EXISTS(SELECT 1 FROM json_each(data,'$.sections') WHERE json_extract(value,'$.topicId')=?)",params![user,key])?;
    crate::daily::enqueue_today(&tx, &user)?;
    crate::persistence::revoke_agent_context(&tx, &user)?;
    tx.commit()?;
    Ok(StatusCode::NO_CONTENT)
}
