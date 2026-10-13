//! User operations create bounded, editable memories without model interpretation.
use crate::{error::Result, now, persistence, topics};
use rusqlite::{params, OptionalExtension, Transaction};
use serde_json::{json, Value};

fn save(tx: &Transaction<'_>, user: &str, mut value: Value) -> Result<()> {
    let key = value["id"].as_str().unwrap_or_default();
    if persistence::tombstoned(tx, user, "cognition", key)? {
        return Ok(());
    }
    let fingerprint = value["fingerprint"].as_str();
    let deleted: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM records WHERE user_id=? AND kind='tombstone' AND json_extract(data,'$.kind')='cognition' AND json_extract(data,'$.fingerprint')=?)",params![user,fingerprint],|r|r.get(0))?;
    if deleted {
        return Ok(());
    }
    let known: Option<String> = tx.query_row("SELECT data FROM records WHERE user_id=? AND kind='cognition' AND (id=? OR (json_extract(data,'$.fingerprint')=? AND (json_extract(data,'$.userEdited')=1 OR json_extract(data,'$.origin')='manual' OR json_extract(data,'$.status')!='active'))) ORDER BY id=? DESC LIMIT 1",params![user,key,fingerprint,key],|r|r.get(0)).optional()?;
    if let Some(raw) = known {
        let owned: Value = serde_json::from_str(&raw)?;
        if owned["userEdited"] == true || owned["origin"] == "manual" || owned["status"] != "active"
        {
            return Ok(());
        }
        if owned["statement"] == value["statement"]
            && owned["evidenceRefs"] == value["evidenceRefs"]
        {
            return Ok(());
        }
        value["id"] = owned["id"].clone();
        value["createdAt"] = owned["createdAt"].clone();
        value["revision"] = json!(owned["revision"].as_u64().unwrap_or(1) + 1);
    }
    persistence::put(tx, user, "cognition", &value)
}

pub(super) fn manual_interest(
    tx: &Transaction<'_>,
    user: &str,
    thought: &Value,
    explicit_content: Option<&Value>,
) -> Result<()> {
    let Some(content) = explicit_content else {
        return Ok(());
    };
    let content: serde_json::Map<String, Value> = ["title", "question", "summary"]
        .into_iter()
        .filter_map(|field| content.get(field).map(|v| (field.to_owned(), v.clone())))
        .collect();
    let content = json!(content);
    let quote = content["question"]
        .as_str()
        .or_else(|| content["title"].as_str())
        .or_else(|| content["summary"].as_str())
        .unwrap_or_default();
    let key = thought["id"].as_str().unwrap_or_default();
    let question = thought["question"].as_str().unwrap_or_default();
    let text = format!("主动研究的问题：{}", crate::clip(question, 1500));
    save(
        tx,
        user,
        json!({
            "id":format!("thought-interest:{key}"),"type":"interest","statement":text,
            "origin":"explicit","status":"active","confidence":1.0,"userEdited":false,
            "fingerprint":topics::fingerprint(&format!("interest:{text}")),
            "evidenceRefs":[{"thoughtId":key,"operation":if thought["revision"] == 1 { "create_thought" } else { "edit_thought" },
                "quote":quote,"explanation":content["summary"],"userInput":content}],
            "inferenceReason":"用户主动新增或编辑此问题，表示希望研究此问题；问题与说明不代表观点或信念。",
            "revision":1,"createdAt":now(),"updatedAt":now()
        }),
    )
}

pub(super) fn deletion_preference(tx: &Transaction<'_>, user: &str, thought: &Value) -> Result<()> {
    let key = thought["id"].as_str().unwrap_or_default();
    let question = thought["question"]
        .as_str()
        .or_else(|| thought["title"].as_str())
        .unwrap_or("此思考");
    let text = format!(
        "少推荐与「{}」相似的思考。此偏好仅限该具体问题，不代表对相关领域不感兴趣。",
        crate::clip(question, 1500)
    );
    save(
        tx,
        user,
        json!({
            "id":format!("thought-delete-preference:{key}"),"type":"preference","statement":text,
            "origin":"explicit","status":"active","confidence":1.0,"userEdited":false,
            "fingerprint":topics::fingerprint(&format!("preference:{text}")),
            "scope":{"kind":"similar_thought","question":question},
            "evidenceRefs":[{"operation":"delete_thought","targetId":key,"preference":"less",
                "quote":"删除，并少推荐与此思考相似的内容","scope":{"question":question}}],
            "inferenceReason":"用户在删除时明确选择少推荐类似思考；这是独立保留的操作偏好，不推断整个领域的兴趣。",
            "revision":1,"createdAt":now(),"updatedAt":now()
        }),
    )
}
