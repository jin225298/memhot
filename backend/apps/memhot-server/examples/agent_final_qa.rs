//! Explicit one-call diagnostic using already retrieved public pages in an isolated QA store.
//! It never publishes a report or mutates the source run. Raw output stays in a private file.
use memhot_server::{agent, models, App};
use serde_json::{json, Value};
use std::{os::unix::fs::PermissionsExt, path::PathBuf};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = PathBuf::from(std::env::var("MEMHOT_FINAL_QA_DIR")?);
    let user = std::env::var("MEMHOT_FINAL_QA_USER")?;
    let operation = std::env::var("MEMHOT_FINAL_QA_OPERATION")?;
    if !dir.to_string_lossy().contains("/qa/") || !operation.starts_with("qa-final-") {
        return Err("explicit QA directory and operation required".into());
    }
    let app = App::open(dir.clone())?;
    let conn = app.store.conn()?;
    let used: i64 = conn.query_row(
        "SELECT coalesce(sum(attempt),0) FROM billing_usage",
        [],
        |r| r.get(0),
    )?;
    if used >= 45 {
        return Err("QA call budget exhausted".into());
    }
    let raw: String = conn.query_row(
        "SELECT data FROM records WHERE user_id=? AND kind='agent_run' AND json_extract(data,'$.input.mode')='research' AND json_array_length(data,'$.context.materials')>0 ORDER BY created DESC LIMIT 1",
        [&user], |r| r.get(0))?;
    drop(conn);
    let saved: Value = serde_json::from_str(&raw)?;
    let pages: Vec<_> = saved["context"]["materials"].as_array().ok_or("missing materials")?
        .iter().filter(|m| m["fulltext"] == true).take(5)
        .map(|m| json!({"evidenceId":memhot_server::hash(m["url"].as_str().unwrap_or_default().as_bytes())[..24].to_owned(),
            "url":m["url"],"title":m["title"],"text":m["text"]})).collect();
    if pages.is_empty() {
        return Err("no retrieved pages".into());
    }
    let scope = agent::Scope::private(&app, &user, None, &operation)?;
    let result = agent::run(&app, &scope, agent::Spec {
        mode: "research", purpose: "qa-final", instructions: saved["input"]["instructions"].as_str().ok_or("missing instructions")?,
        prompt: json!({"task":"根据已读官方页面完成研究结论，资料不足如实标注。","originalRequest":saved["input"]["prompt"],"readPages":pages}),
        max_steps: 1, max_output_tokens: 8192, tools: false,
    }, agent::ToolContext::default()).await;
    let path = dir.join(format!("{operation}.json"));
    let value = match result {
        Ok(output) => {
            let parsed = models::parse_json(&output.text);
            println!(
                "{}",
                json!({"status":"completed","validJson":parsed.is_ok(),"claims":parsed.as_ref().ok().and_then(|v|v["claims"].as_array()).map(Vec::len),"steps":output.steps})
            );
            json!({"text":output.text,"parsed":parsed.ok()})
        }
        Err(error) => {
            println!("{}", json!({"status":"failed","code":error.code}));
            json!({"error":error.code})
        }
    };
    std::fs::write(&path, serde_json::to_vec(&value)?)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}
