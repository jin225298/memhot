//! Operator-only paid end-to-end check using fixed public AI sources in an isolated store.
//! No user records are read. Requires an explicit fresh MEMHOT_REPORT_QA_DIR.
use memhot_server::{shared_public, App};
use serde_json::json;
use std::{os::unix::fs::PermissionsExt, path::PathBuf};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let directory = PathBuf::from(std::env::var("MEMHOT_REPORT_QA_DIR")?);
    if !directory.is_absolute() || directory.exists() {
        return Err("choose a fresh absolute QA directory".into());
    }
    std::fs::create_dir(&directory)?;
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
    let app = App::open(directory.clone())?;
    let result = shared_public::edition(&app, "ai", "zh-CN").await?;
    let attempts: i64 =
        app.store
            .conn()?
            .query_row("SELECT count(*) FROM public_model_attempts", [], |row| {
                row.get(0)
            })?;
    let report = result.edition;
    let outcome = json!({"cacheStatus":result.cache_status,"warning":result.warning,
        "modelAttempts":attempts,"hasReport":report.is_some(),
        "stories":report.as_ref().map(|r|r["editorial"]["acceptedStories"].clone()),
        "overviewStatus":report.as_ref().map(|r|r["editorial"]["overviewStatus"].clone()),
        "sourceCount":report.as_ref().and_then(|r|r["sources"].as_array()).map(Vec::len)});
    for (name, value) in [
        ("outcome.json", outcome.clone()),
        ("report.json", json!(report)),
    ] {
        let path = directory.join(name);
        std::fs::write(&path, serde_json::to_string_pretty(&value)?)?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    println!("{outcome}");
    if report.is_none() {
        return Err("no verified report; inspect private QA evidence".into());
    }
    Ok(())
}
