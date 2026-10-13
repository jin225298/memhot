//! Read retained evidence only. This module never fetches a user-supplied URL.
use crate::{
    auth::User,
    error::{Error, Result},
    App,
};
use axum::{
    extract::{Path, State},
    Json,
};
use rusqlite::{params, OptionalExtension, Transaction};
use serde_json::{json, Value};

pub(super) struct Snapshot {
    pub value: Value,
    pub report_id: String,
    pub evidence_id: String,
    pub report_hash: String,
    pub evidence_hash: String,
    pub expires_at: i64,
    edition: Option<(String, String)>,
    cache: Option<(bool, String, String)>,
}
impl Snapshot {
    pub fn text(&self) -> &str {
        self.value["text"].as_str().unwrap_or_default()
    }
    pub fn content_hash(&self) -> String {
        crate::hash(
            json!([
                self.text(),
                self.value["availability"],
                self.value["retrievalKind"],
                self.value["truncated"],
                self.expires_at
            ])
            .to_string()
            .as_bytes(),
        )
    }
}
pub(super) fn load(app: &App, user: &str, report_id: &str, evidence_id: &str) -> Result<Snapshot> {
    let report = crate::report_access::get(app, user, report_id)?;
    let evidence = report["sources"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|source| source["id"] == evidence_id)
        .ok_or_else(Error::missing)?;
    let reference = app.store.get(user, "report", report_id)?;
    let at = chrono::Utc::now().timestamp();
    let url = evidence["url"].as_str().unwrap_or_default();
    let mut text = evidence["excerpt"].as_str().unwrap_or_default().to_owned();
    let mut expires_at = 0; // Report quotations may outlive the short-lived original page.
    let mut cache = None;
    let mut edition = None;
    let mut fresh = false;
    let mut upstream_truncated = false;
    if let Some(edition_id) = reference["publicEditionId"].as_str() {
        if report["publicEditionId"] != edition_id {
            return Err(Error::missing());
        }
        let conn = app.store.conn()?;
        let body: Option<String> = conn
            .query_row(
                "SELECT body FROM public_cache WHERE kind='edition' AND cache_key=?",
                [edition_id],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        edition = body.map(|body| (edition_id.to_owned(), crate::hash(body.as_bytes())));
        // A redirect may have changed the cache key; the exact authorized final URL must match.
        let row: Option<(String, String, i64)> = conn.query_row("SELECT cache_key,body,retain_until FROM public_cache WHERE kind='page' AND body IS NOT NULL AND json_extract(body,'$.url')=? ORDER BY json_extract(body,'$.fetchedAt') DESC LIMIT 1", [url], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
        if let Some((key, body, retain)) = row {
            let page: Value = serde_json::from_str(&body)?;
            let fetched = page["fetchedAt"].as_i64().unwrap_or(0);
            if retain > at && fetched > at - 86400 && fetched <= at && page["url"] == url {
                let raw = page["text"].as_str().unwrap_or_default();
                let plain = if page["contentType"]
                    .as_str()
                    .is_some_and(|s| s.contains("html"))
                {
                    crate::discovery::plain(raw)
                } else {
                    raw.to_owned()
                };
                if !plain.trim().is_empty() {
                    text = plain;
                    fresh = true;
                    expires_at = retain.min(fetched + 86400);
                    cache = Some((true, key, crate::hash(body.as_bytes())));
                }
            }
        }
    } else if let Ok(raw) = app.store.get(user, "raw", &crate::hash(url.as_bytes())) {
        let fetched = raw["createdAt"]
            .as_str()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|at| at.timestamp())
            .unwrap_or(0);
        if raw["url"] == url
            && fetched <= at
            && fetched > at - 86400
            && raw["expiresAt"]
                .as_i64()
                .is_some_and(|expires| expires > at)
            && raw["text"].as_str().is_some_and(|s| !s.trim().is_empty())
        {
            text = raw["text"].as_str().unwrap_or_default().to_owned();
            // Older private retrieval capped stored input at 18k without recording its original length.
            upstream_truncated = raw["truncated"] == true || text.chars().count() >= 18000;
            fresh = true;
            expires_at = raw["expiresAt"].as_i64().unwrap_or(at).min(fetched + 86400);
            cache = Some((
                false,
                raw["id"].as_str().unwrap_or_default().into(),
                crate::hash(raw.to_string().as_bytes()),
            ));
        }
    }
    let page = fresh && evidence["retrievalKind"] == "page";
    let truncated = upstream_truncated || text.chars().count() > 12000;
    let mut value = json!({"evidenceId":evidence_id,"title":evidence["title"],"url":evidence["url"],"publisher":evidence["publisher"],"attribution":evidence["attribution"],"text":crate::clip(&text,12000),"retrievalKind":if page {"page"} else {"metadata_or_abstract"},"availability":if page {"full"} else {"excerpt"},"truncated":truncated});
    if let Some(published) = evidence.get("publishedAt").filter(|v| !v.is_null()) {
        value["publishedAt"] = published.clone();
    }
    if truncated {
        value["message"] = json!("材料较长，当前展示并翻译保存的前12000字；其余内容请访问原文。");
    } else if !fresh {
        value["message"] = json!("原文缓存已过期或未保存，当前仅展示报告保留的引文摘要。");
    } else if !page {
        value["message"] = json!("当前材料是元数据或摘要，不代表已读取完整原文。");
    }
    Ok(Snapshot {
        value,
        report_id: report_id.into(),
        evidence_id: evidence_id.into(),
        report_hash: crate::hash(reference.to_string().as_bytes()),
        evidence_hash: crate::hash(evidence.to_string().as_bytes()),
        expires_at,
        edition,
        cache,
    })
}
/// Validate the same authorization and cached input under the queue/commit write lock.
pub(super) fn validate(tx: &Transaction<'_>, user: &str, snapshot: &Snapshot) -> Result<()> {
    let report = crate::persistence::current(tx, user, "report", &snapshot.report_id)?
        .filter(crate::research::available)
        .ok_or_else(Error::missing)?;
    if crate::hash(report.to_string().as_bytes()) != snapshot.report_hash {
        return Err(Error::new(
            axum::http::StatusCode::CONFLICT,
            "translation_context_changed",
            "报告已发生变化，请刷新材料后重试",
        ));
    }
    if let Some((key, expected)) = &snapshot.edition {
        let body: Option<String> = tx
            .query_row(
                "SELECT body FROM public_cache WHERE kind='edition' AND cache_key=?",
                [key],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        if body.is_none_or(|body| crate::hash(body.as_bytes()) != *expected) {
            return Err(Error::missing());
        }
    }
    if let Some((public, key, expected)) = &snapshot.cache {
        let body: Option<String> = if *public {
            tx.query_row("SELECT body FROM public_cache WHERE kind='page' AND cache_key=? AND retain_until>?",params![key,chrono::Utc::now().timestamp()],|r|r.get(0)).optional()?.flatten()
        } else {
            tx.query_row("SELECT data FROM records WHERE user_id=? AND kind='raw' AND id=? AND json_extract(data,'$.expiresAt')>?",params![user,key,chrono::Utc::now().timestamp()],|r|r.get(0)).optional()?
        };
        let current_hash = body
            .map(|body| -> Result<String> {
                if *public {
                    Ok(crate::hash(body.as_bytes()))
                } else {
                    Ok(crate::hash(
                        serde_json::from_str::<Value>(&body)?.to_string().as_bytes(),
                    ))
                }
            })
            .transpose()?;
        if chrono::Utc::now().timestamp() >= snapshot.expires_at
            || current_hash.as_ref() != Some(expected)
        {
            return Err(Error::new(
                axum::http::StatusCode::CONFLICT,
                "translation_material_expired",
                "原文缓存已过期或变化，请刷新后翻译当前可用摘要",
            ));
        }
    }
    Ok(())
}
pub async fn material(
    State(app): State<App>,
    User(user): User,
    Path((report, evidence)): Path<(String, String)>,
) -> Result<Json<Value>> {
    let snapshot = load(&app, &user, &report, &evidence)?;
    let mut value = snapshot.value.clone();
    if let Some(translation) = super::translation::state(&app, &user, &snapshot)? {
        value["translation"] = translation;
    }
    Ok(Json(value))
}
