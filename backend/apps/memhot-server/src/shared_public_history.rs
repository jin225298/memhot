//! Shared edition memory only reads operator-owned public artifacts, never profiles.
use crate::{error::Result, App};
use rusqlite::params;
use serde_json::Value;
use std::collections::HashSet;
pub(crate) fn seen(app: &App, topic: &str) -> Result<HashSet<String>> {
    let conn = app.store.conn()?;
    let mut stmt = conn.prepare("SELECT body FROM public_cache WHERE kind='edition' AND body IS NOT NULL AND json_extract(body,'$.catalogTopicId')=? AND julianday(json_extract(body,'$.createdAt'))>=julianday('now')-7 ORDER BY json_extract(body,'$.createdAt') DESC LIMIT 28")?;
    let rows = stmt.query_map(params![topic], |r| r.get::<_, String>(0))?;
    let mut seen = HashSet::new();
    for row in rows {
        let report: Value = serde_json::from_str(&row?)?;
        for source in report["sources"].as_array().into_iter().flatten() {
            if let Some(url) = source["url"].as_str() {
                seen.insert(crate::freshness::canonical(url));
            }
        }
    }
    Ok(seen)
}
