//! Explicit public-only MCP smoke check. No model, provider credentials, or user data.
use memhot_server::{aihot, shared_public, App};
use serde_json::json;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    if !aihot::enabled() {
        return Err("set MEMHOT_AIHOT_ENABLED=1 only for authorized AIHOT use".into());
    }
    let directory = std::env::temp_dir().join(format!("memhot-aihot-qa-{}", memhot_server::id()));
    let app = App::open(directory.clone())?;
    let first = shared_public::aihot_cached(&app, "ai", "daily").await?;
    let decoded = aihot::decode(&first)?;
    let (key, expires, lease): (String, i64, i64) = app.store.conn()?.query_row(
        "SELECT cache_key,expires_at,lease_until FROM public_cache WHERE kind='aihot'",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    assert!(expires > chrono::Utc::now().timestamp() && lease == 0);
    let second = shared_public::aihot_cached(&app, "ai", "daily").await?;
    assert_eq!(first, second);
    let count: i64 = app.store.conn()?.query_row(
        "SELECT count(*) FROM public_cache WHERE kind='aihot'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(count, 1);
    println!(
        "{}",
        json!({"mcpTool":"aihot_get_daily","sourceCount":decoded.materials.len(),
        "hasAttributedOverview":decoded.overview.as_ref().is_some_and(|v|v["attribution"]["name"] == "AIHOT"),
        "allDiscoveriesNotFulltext":decoded.materials.iter().all(|m| m.is_search_snippet() && !m.fulltext),
        "allHaveAiHotAttribution":decoded.materials.iter().all(|m|m.discovered_via.as_deref().is_some_and(|v|v.starts_with("https://aihot.news/"))),
        "samePublicCacheKeyHit":first == second,"cacheKey":key})
    );
    drop(app);
    std::fs::remove_dir_all(directory)?;
    Ok(())
}
