use crate::{
    auth::User,
    error::{Error, Result},
    network, App,
};
use axum::{extract::State, Json};
use serde_json::{json, Value};

pub fn default_profile() -> Value {
    json!({"name":"我","interests":[],"timezone":"Asia/Shanghai","researchHour":23,"digestHour":8,"retentionDays":30,"models":{},"feeds":[],"search":{"provider":"public","configured":false}})
}
pub fn public_profile(p: &Value) -> Value {
    let mut p = p.clone();
    // Legacy feed URLs can embed credentials; canonical subscriptions expose only a host.
    p["feeds"] = json!([]);
    for kind in ["text", "asr"] {
        if let Some(config) = p.get_mut("models").and_then(|m| m.get_mut(kind)) {
            let configured = super::models::configured(config);
            if let Some(map) = config.as_object_mut() {
                map.remove("apiKey");
                map.insert("configured".into(), json!(configured));
            }
        }
    }
    if let Some(search) = p.get_mut("search").and_then(Value::as_object_mut) {
        let configured = search
            .get("apiKey")
            .and_then(Value::as_str)
            .is_some_and(|s| !s.is_empty());
        search.remove("apiKey");
        search.insert("configured".into(), json!(configured));
    }
    p
}
pub async fn get(State(app): State<App>, User(user): User) -> Result<Json<Value>> {
    Ok(Json(snapshot(&app, &user)?))
}
pub fn snapshot(app: &App, user: &str) -> Result<Value> {
    crate::subscription_store::migrate_legacy(app, user)?;
    let mut p = public_profile(&app.store.profile(user)?);
    p["feeds"] = json!(crate::subscription_store::legacy_feeds(app, user)?);
    Ok(p)
}
pub async fn update(
    State(app): State<App>,
    User(user): User,
    Json(patch): Json<Value>,
) -> Result<Json<Value>> {
    crate::subscription_store::migrate_legacy(&app, &user)?;
    let mut p = app.store.profile(&user)?;
    let original = p.to_string();
    let map = patch
        .as_object()
        .ok_or_else(|| Error::bad("设置必须是JSON对象"))?;
    for (key, value) in map {
        match key.as_str() {
            "name" => {
                let text = value
                    .as_str()
                    .filter(|s| !s.trim().is_empty() && s.chars().count() <= 80)
                    .ok_or_else(|| Error::bad("名字须为1至80字"))?;
                p[key] = json!(text);
            }
            "timezone" => {
                let timezone = value.as_str().ok_or_else(|| Error::bad("时区无效"))?;
                timezone
                    .parse::<chrono_tz::Tz>()
                    .map_err(|_| Error::bad("时区无效"))?;
                p[key] = value.clone();
            }
            "digestHour" | "researchHour" => {
                if value.as_u64().is_none_or(|n| n > 23) {
                    return Err(Error::bad("出刊小时须在0至23之间"));
                }
                p[key] = value.clone();
            }
            "retentionDays" => {
                if value.as_u64().is_none_or(|n| !(1..=365).contains(&n)) {
                    return Err(Error::bad("保留天数须在1至365之间"));
                }
                p[key] = value.clone();
            }
            "interests" | "feeds" => {
                let items = value
                    .as_array()
                    .filter(|a| a.len() <= 20)
                    .ok_or_else(|| Error::bad("最多设置20项"))?;
                for item in items {
                    let text = item
                        .as_str()
                        .filter(|s| !s.is_empty() && s.len() <= 2048)
                        .ok_or_else(|| Error::bad("设置条目无效"))?;
                    if key == "feeds" {
                        network::validate_url(text)?;
                    }
                }
                p[key] = value.clone();
            }
            "models" => {
                let models = value
                    .as_object()
                    .ok_or_else(|| Error::bad("模型设置无效"))?;
                for (kind, config) in models {
                    if !matches!(kind.as_str(), "text" | "asr") {
                        return Err(Error::bad("模型类型无效"));
                    }
                    if config.is_null() {
                        p["models"][kind] = Value::Null;
                        continue;
                    }
                    let config = config
                        .as_object()
                        .ok_or_else(|| Error::bad("模型设置无效"))?;
                    let previous = &p["models"][kind];
                    let mut merged = previous.as_object().cloned().unwrap_or_default();
                    let changed_destination = ["baseUrl", "protocol"].iter().any(|field| {
                        config.get(*field).is_some_and(|next| {
                            let old = previous[*field]
                                .as_str()
                                .unwrap_or(if *field == "protocol" { "openai" } else { "" });
                            let next = next.as_str().unwrap_or_default();
                            if *field == "baseUrl" {
                                old.trim_end_matches('/') != next.trim_end_matches('/')
                            } else {
                                old != next
                            }
                        })
                    });
                    if changed_destination && !config.contains_key("apiKey") {
                        merged.remove("apiKey");
                    }
                    for (key, v) in config {
                        if !matches!(key.as_str(), "protocol" | "baseUrl" | "model" | "apiKey") {
                            continue;
                        }
                        if !v.is_string() || v.as_str().unwrap_or_default().len() > 8192 {
                            return Err(Error::bad("模型字段无效"));
                        }
                        merged.insert(key.clone(), v.clone());
                    }
                    let v = Value::Object(merged);
                    validate_model(kind, &v)?;
                    p["models"][kind] = v;
                }
            }
            "search" => {
                let s = value
                    .as_object()
                    .ok_or_else(|| Error::bad("搜索设置无效"))?;
                if let Some(provider) = s.get("provider") {
                    if !matches!(provider.as_str(), Some("public" | "tavily" | "exa")) {
                        return Err(Error::bad("搜索服务无效"));
                    }
                    if &p["search"]["provider"] != provider && !s.contains_key("apiKey") {
                        p["search"].as_object_mut().map(|v| v.remove("apiKey"));
                    }
                    p["search"]["provider"] = provider.clone();
                }
                if let Some(key) = s.get("apiKey") {
                    if !key.is_string() || key.as_str().unwrap_or_default().len() > 8192 {
                        return Err(Error::bad("搜索密钥无效"));
                    }
                    p["search"]["apiKey"] = key.clone();
                }
            }
            _ => return Err(Error::bad("未知设置字段")),
        }
    }
    p["feeds"] = json!([]);
    crate::subscription_store::update_profile(&app, &user, &original, &p, map.get("feeds"))?;
    if map.contains_key("models")
        && (crate::models::configured(&p["models"]["text"])
            || crate::models::configured(&p["models"]["asr"]))
    {
        app.store.conn()?.execute("UPDATE jobs SET status='queued',attempt=0,next_at=0 WHERE user_id=? AND status='failed' AND kind IN ('transcribe','formation') AND (kind='formation' OR EXISTS(SELECT 1 FROM records WHERE user_id=jobs.user_id AND kind='source' AND id=json_extract(jobs.payload,'$.sourceId'))) AND NOT EXISTS(SELECT 1 FROM jobs child WHERE child.user_id=jobs.user_id AND child.dedup='retry:'||jobs.id) AND NOT EXISTS(SELECT 1 FROM receipts WHERE user_id=jobs.user_id AND job_id=jobs.id AND status IN ('pending','paid_incomplete'))",[&user])?;
        for source in app
            .store
            .list_all(&user, "source")?
            .into_iter()
            .filter(|s| s["status"] == "ready" && s["analysisStatus"] == "model_unconfigured")
        {
            let source_id = source["id"].as_str().ok_or_else(Error::internal)?;
            let thought_id = app
                .store
                .list_all(&user, "thought")?
                .into_iter()
                .find(|t| t["sourceId"] == source_id)
                .map(|t| t["id"].clone());
            app.store.queue(
                &user,
                "formation",
                json!({"sourceId":source_id,"thoughtId":thought_id}),
                &format!("formation:{source_id}:reconfigure:{}", crate::id()),
            )?;
        }
    }

    Ok(Json(snapshot(&app, &user)?))
}
fn validate_model(kind: &str, v: &Value) -> Result<()> {
    if let Some(url) = v["baseUrl"].as_str().filter(|s| !s.is_empty()) {
        let url = network::validate_url(url)?;
        if url.scheme() != "https" {
            return Err(Error::bad("模型地址必须使用HTTPS"));
        }
    }
    let protocol = v["protocol"].as_str().unwrap_or("openai");
    if (kind == "text" && !matches!(protocol, "openai" | "anthropic" | "gemini"))
        || (kind == "asr" && !matches!(protocol, "openai" | "qwen" | "mimo"))
    {
        return Err(Error::bad("不支持此模型协议"));
    }
    Ok(())
}
