//! Atomic, one-time preference setup. Existing configuration and manual cognition survive.
use crate::{
    auth::User,
    error::{Error, Result},
    now, persistence, topics, App,
};
use axum::{extract::State, Json};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

const INTERESTS: &[(&str, &str)] = &[
    ("ai", "人工智能"),
    ("robotics", "机器人"),
    ("astronomy", "天文学"),
    ("physics", "物理学"),
    ("product", "产品设计"),
    ("business", "商业与创业"),
    ("technology", "科技产业"),
    ("psychology", "心理与认知"),
    ("history", "历史与人文"),
    ("economy", "经济与社会"),
    ("learning", "教育学习"),
    ("health", "健康与生活"),
];
const READING: &[(&str, &str, &str)] = &[
    (
        "summary",
        "先看结论",
        "我偏好先看清楚的结论与关键判断，再展开阅读依据。",
    ),
    (
        "data",
        "数据与图表",
        "我偏好通过数据与图表理解问题，并了解数据的来源和适用范围。",
    ),
    (
        "reports",
        "原始报告",
        "我偏好阅读原始报告和一手资料，能够追溯结论的出处。",
    ),
    (
        "depth",
        "深入解读",
        "我偏好深入解读问题的背景、机制和限制，不只看简短摘要。",
    ),
    (
        "cases",
        "真实案例",
        "我偏好用真实案例理解问题，并区分个案经验与可推广的结论。",
    ),
];
const THINKING: &[(&str, &str, &str)] = &[
    (
        "counter",
        "补充反例",
        "我希望分析想法时主动补充反例与相反证据。",
    ),
    (
        "compare",
        "比较不同解释",
        "我希望比较不同解释和可选方案，明确各自的依据与限制。",
    ),
    (
        "action",
        "可行的下一步",
        "我希望讨论能提出具体可行的下一步验证方法。",
    ),
    (
        "longterm",
        "追踪长期变化",
        "我希望追踪长期变化与持续出现的模式，避免只关注短期热点。",
    ),
];
fn response(completed: bool) -> Value {
    let preferences = |values: &[(&str, &str, &str)]| {
        values
            .iter()
            .map(|(id, label, statement)| json!({"id":id,"label":label,"statement":statement}))
            .collect::<Vec<_>>()
    };
    json!({"completed":completed,"interests":INTERESTS.iter().map(|(id,label)|json!({"id":id,"label":label,"value":label})).collect::<Vec<_>>(),"reading":preferences(READING),"thinking":preferences(THINKING)})
}
fn profile(conn: &Connection, user: &str) -> Result<Value> {
    let raw: Option<String> = conn
        .query_row("SELECT profile FROM users WHERE id=?", [user], |r| r.get(0))
        .optional()?;
    Ok(serde_json::from_str(&raw.ok_or_else(Error::auth)?)?)
}
fn completed(conn: &Connection, user: &str, p: &Value) -> Result<bool> {
    if p["preferencesOnboarded"] == true
        || p["interests"].as_array().is_some_and(|items| {
            items
                .iter()
                .any(|v| v.as_str().is_some_and(|s| !s.trim().is_empty()))
        })
    {
        return Ok(true);
    }
    Ok(conn.query_row("SELECT EXISTS(SELECT 1 FROM records WHERE user_id=? AND kind='cognition' AND json_extract(data,'$.status') IN ('active','pending') AND json_extract(data,'$.type') IN ('interest','preference','goal','belief','constraint') AND json_type(data,'$.statement')='text' AND length(json_extract(data,'$.statement'))<=2000 AND length(trim(COALESCE(json_extract(data,'$.statement'),'')))>0)",[user],|r|r.get(0))?)
}
pub async fn get(State(app): State<App>, User(user): User) -> Result<Json<Value>> {
    let conn = app.store.conn()?;
    Ok(Json(response(completed(
        &conn,
        &user,
        &profile(&conn, &user)?,
    )?)))
}
fn strings(input: &Value, key: &str, limit: usize, max_chars: usize) -> Result<Vec<String>> {
    let Some(value) = input.get(key) else {
        return Ok(Vec::new());
    };
    let values = value
        .as_array()
        .filter(|a| a.len() <= limit)
        .ok_or_else(|| Error::bad(format!("{key}须为不超过{limit}项的列表")))?;
    values
        .iter()
        .map(|v| {
            v.as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty() && s.chars().count() <= max_chars)
                .map(str::to_owned)
                .ok_or_else(|| Error::bad(format!("{key}条目须为1至{max_chars}字")))
        })
        .collect()
}
fn selections(input: &Value, key: &str, options: &[(&str, &str, &str)]) -> Result<Vec<String>> {
    let ids = strings(input, key, options.len(), 32)?;
    if ids
        .iter()
        .any(|id| !options.iter().any(|(key, _, _)| key == id))
    {
        return Err(Error::bad(format!("{key}含未知选项")));
    }
    Ok(ids)
}
pub async fn submit(
    State(app): State<App>,
    User(user): User,
    Json(input): Json<Value>,
) -> Result<Json<Value>> {
    let map = input
        .as_object()
        .ok_or_else(|| Error::bad("偏好设置须为JSON对象"))?;
    if map.keys().any(|k| {
        !matches!(
            k.as_str(),
            "interestIds" | "customInterests" | "readingIds" | "thinkingIds" | "skip"
        )
    }) {
        return Err(Error::bad("未知偏好设置字段"));
    }
    let skip = match input.get("skip") {
        None => false,
        Some(v) => v.as_bool().ok_or_else(|| Error::bad("skip须为布尔值"))?,
    };
    let ids = strings(&input, "interestIds", INTERESTS.len(), 32)?;
    if ids
        .iter()
        .any(|id| !INTERESTS.iter().any(|(key, _)| key == id))
    {
        return Err(Error::bad("interestIds含未知选项"));
    }
    let custom = strings(&input, "customInterests", 20, 80)?;
    let reading = selections(&input, "readingIds", READING)?;
    let thinking = selections(&input, "thinkingIds", THINKING)?;
    let mut conn = app.store.conn()?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let mut p = profile(&tx, &user)?;
    if p["preferencesOnboarded"] == true {
        return Ok(Json(response(true)));
    }
    if !skip {
        let mut interests: Vec<Value> = Vec::new();
        for value in p["interests"].as_array().into_iter().flatten() {
            let Some(text) = value.as_str().filter(|s| !s.trim().is_empty()) else {
                continue;
            };
            if !interests.iter().any(|v| {
                v.as_str()
                    .is_some_and(|s| s.trim().to_lowercase() == text.trim().to_lowercase())
            }) {
                interests.push(value.clone());
            }
        }
        for value in ids
            .iter()
            .filter_map(|id| {
                INTERESTS
                    .iter()
                    .find(|(key, _)| key == id)
                    .map(|(_, label)| (*label).to_owned())
            })
            .chain(custom)
        {
            if !interests.iter().any(|v| {
                v.as_str()
                    .is_some_and(|s| s.trim().to_lowercase() == value.to_lowercase())
            }) {
                interests.push(json!(value));
            }
        }
        if interests.len() > 20 {
            return Err(Error::bad("合并后最多保留20个兴趣"));
        }
        p["interests"] = json!(interests);
        for (group, ids, options) in [
            ("reading", reading, READING),
            ("thinking", thinking, THINKING),
        ] {
            for id in ids {
                let (_, _, statement) = options
                    .iter()
                    .find(|(key, _, _)| *key == id)
                    .ok_or_else(Error::internal)?;
                let key = format!("setup-preference-{group}-{id}");
                if persistence::current(&tx, &user, "cognition", &key)?.is_some()
                    || persistence::tombstoned(&tx, &user, "cognition", &key)?
                {
                    continue;
                }
                let value = json!({"id":key,"type":"preference","statement":statement,"origin":"manual","status":"active","confidence":1.0,"evidenceRefs":[],"inferenceReason":"用户在首次偏好设置中主动选择","revision":1,"userEdited":true,"fingerprint":topics::fingerprint(&format!("preference:{statement}")),"createdAt":now(),"updatedAt":now()});
                persistence::put(&tx, &user, "cognition", &value)?;
            }
        }
    }
    p["preferencesOnboarded"] = json!(true);
    tx.execute(
        "UPDATE users SET profile=? WHERE id=?",
        params![p.to_string(), user],
    )?;
    tx.commit()?;
    Ok(Json(response(true)))
}
