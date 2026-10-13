//! Only owned, delivered references are hydrated with public editorial content.
use crate::{
    error::{Error, Result},
    research, shared_public, App,
};
use serde_json::{json, Value};

pub fn get(app: &App, user: &str, key: &str) -> Result<Value> {
    let report = app.store.get(user, "report", key)?;
    if !research::available(&report) {
        return Err(Error::missing());
    }
    hydrate(app, report)
}
pub fn list(app: &App, user: &str) -> Result<Vec<Value>> {
    app.store
        .list(user, "report")?
        .into_iter()
        .filter(research::available)
        .map(|report| hydrate(app, report))
        .collect()
}
fn hydrate(app: &App, reference: Value) -> Result<Value> {
    let Some(edition_id) = reference["publicEditionId"].as_str() else {
        return Ok(reference);
    };
    let mut report = match shared_public::get(app, edition_id)? {
        Some(report) => report,
        None => {
            json!({"title":"公共日报暂不可用","summary":"公开汇编已过保留期或暂不可读取。","body":"","sources":[],"gaps":["公开汇编暂不可用，请刷新后重试"],"status":"partial","cacheStatus":"unavailable"})
        }
    };
    let current = report["cacheStatus"].as_str().unwrap_or("ready").to_owned();
    if !matches!(current.as_str(), "expired" | "unavailable" | "stale")
        && reference["publicCacheStatus"] == "stale"
    {
        report["cacheStatus"] = json!("stale");
    } else {
        report["cacheStatus"] = json!(current);
    }
    if reference["publicCacheStatus"] == "unchanged"
        && !matches!(current.as_str(), "expired" | "unavailable")
    {
        report["cacheStatus"] = json!("unchanged");
        let mut gaps = report["gaps"].as_array().cloned().unwrap_or_default();
        if let Some(note) = reference["publicWarning"].as_str() {
            if !gaps.iter().any(|g| g.as_str() == Some(note)) {
                gaps.push(json!(note));
            }
        }
        report["gaps"] = json!(gaps);
    }
    let stale = matches!(
        report["cacheStatus"].as_str(),
        Some("stale" | "expired" | "unavailable")
    );
    if stale {
        report["status"] = json!("partial");
        let mut gaps = report["gaps"].as_array().cloned().unwrap_or_default();
        let note = reference["publicWarning"]
            .as_str()
            .unwrap_or("正在展示带日期的缓存材料，未将旧内容视为今天的新进展");
        if !gaps.iter().any(|g| g.as_str() == Some(note)) {
            gaps.push(json!(note));
        }
        report["gaps"] = json!(gaps);
    }
    for field in [
        "id",
        "kind",
        "createdAt",
        "availableAt",
        "researchDate",
        "localDate",
        "publicEditionId",
        "catalogTopicId",
    ] {
        if let Some(value) = reference.get(field) {
            if field == "createdAt" && report["createdAt"].is_string() {
                continue;
            }
            report[field] = value.clone();
        } else if field == "availableAt" {
            report
                .as_object_mut()
                .ok_or_else(Error::internal)?
                .remove(field);
        }
    }
    report["sharedPublic"] = json!(true);
    Ok(report)
}
