//! Retain only read, cited pages. An earlier search snippet must never replace a page.
use crate::{db::Job, discovery::Material, hash, now};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashSet};

pub(super) fn report_records(
    mut report: Value,
    job: &Job,
    materials: &[Material],
    epoch: i64,
) -> Vec<(&'static str, Value)> {
    report["_userEpoch"] = json!(epoch);
    for field in ["availableAt", "researchDate", "localDate"] {
        if let Some(value) = job.payload.get(field) {
            report[field] = value.clone();
        }
    }
    let agent_raw = report
        .as_object_mut()
        .and_then(|r| r.remove("_agentRaw"))
        .and_then(|r| r.as_array().cloned())
        .unwrap_or_default();
    let cited: HashSet<String> = report["sources"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|source| source["url"].as_str().map(str::to_owned))
        .collect();
    let mut pages = BTreeMap::new();
    for material in materials
        .iter()
        .filter(|m| m.fulltext && cited.contains(&m.url))
    {
        pages.insert(material.url.clone(), material.text.clone());
    }
    // The final agent inventory supersedes initial retrieval, including updated page text.
    for raw in agent_raw.iter().filter(|r| r["fulltext"] == true) {
        if let (Some(url), Some(text)) = (raw["url"].as_str(), raw["text"].as_str()) {
            if cited.contains(url) && !text.trim().is_empty() {
                pages.insert(url.to_owned(), text.to_owned());
            }
        }
    }
    let mut records = vec![("report", report)];
    for (url, text) in pages {
        records.push((
            "raw",
            json!({"id":hash(url.as_bytes()),"_userEpoch":epoch,
            "createdAt":now(),"expiresAt":chrono::Utc::now().timestamp()+86400,
            "text":text,"url":url,"fulltext":true,"contentHash":hash(text.as_bytes())}),
        ));
    }
    records
}

#[cfg(test)]
mod tests {
    use super::*;
    fn material(url: &str, fulltext: bool) -> Material {
        Material {
            title: "Test".into(),
            url: url.into(),
            text: "Initial text".into(),
            published: None,
            discovered: None,
            discovered_via: None,
            publication_source: "search_reported_publication",
            publisher: "Test".into(),
            fulltext,
            pdf: None,
        }
    }
    #[test]
    fn final_read_page_wins_and_unread_or_uncited_material_is_not_cached() {
        let job = Job {
            id: "j".into(),
            user: "u".into(),
            kind: "digest".into(),
            payload: json!({}),
            attempt: 0,
        };
        let report = json!({"id":"r","sources":[{"url":"https://example.org/read"},
            {"url":"https://example.org/snippet"}],"_agentRaw":[
            {"url":"https://example.org/read","text":"Final verified page","fulltext":true},
            {"url":"https://example.org/snippet","text":"Search result","fulltext":false}]});
        let records = report_records(
            report,
            &job,
            &[
                material("https://example.org/read", true),
                material("https://example.org/snippet", false),
                material("https://example.org/unrelated", true),
            ],
            4,
        );
        assert_eq!(records.len(), 2);
        assert!(records[0].1.get("_agentRaw").is_none());
        assert_eq!(records[1].1["text"], "Final verified page");
        assert_eq!(records[1].1["_userEpoch"], 4);
        assert_eq!(records[1].1["fulltext"], true);
    }
}
