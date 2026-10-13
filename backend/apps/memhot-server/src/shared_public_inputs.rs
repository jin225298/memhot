//! An edition's first verified input is immutable across model retries and server restarts.
use super::{catalog_data, retrieval, CatalogTopic, Lease, PIPELINE_VERSION};
use crate::{
    discovery::Material,
    error::{Error, Result},
    hash, now, App,
};
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

pub(super) fn migrate(conn: &Connection) -> Result<()> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS public_editorial_inputs(edition_key TEXT PRIMARY KEY,body TEXT,fingerprint TEXT NOT NULL,created INTEGER NOT NULL,expires INTEGER NOT NULL);")?;
    Ok(())
}
fn load(app: &App, key: &str) -> Result<Option<Value>> {
    let row: Option<(Option<String>, String, i64)> = app
        .store
        .conn()?
        .query_row(
            "SELECT body,fingerprint,expires FROM public_editorial_inputs WHERE edition_key=?",
            [key],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    row.map(|(body, fingerprint, expires)| {
        let body = body
            .filter(|_| expires > Utc::now().timestamp())
            .ok_or_else(|| {
                Error::upstream(
                    "public_inputs_expired",
                    "公共编辑的固定输入已过保留期，不能重新抓取并重放原调用",
                )
            })?;
        if hash(body.as_bytes()) != fingerprint {
            return Err(Error::upstream(
                "public_inputs_changed",
                "公共编辑输入校验失败，已阻止重复调用",
            ));
        }
        serde_json::from_str(&body).map_err(Into::into)
    })
    .transpose()
}
fn freeze(app: &App, lease: &Lease, value: &Value) -> Result<Value> {
    let body = value.to_string();
    let at = Utc::now().timestamp();
    let conn = app.store.conn()?;
    // The lease token fences a late retrieval from overwriting a subsequent writer's snapshot.
    conn.execute("INSERT OR IGNORE INTO public_editorial_inputs(edition_key,body,fingerprint,created,expires) SELECT ?,?,?,?,? WHERE EXISTS(SELECT 1 FROM public_cache WHERE cache_key=? AND lease_token=? AND lease_until>?)",params![lease.key,body,hash(body.as_bytes()),at,at+86400,lease.key,lease.token,at])?;
    drop(conn);
    load(app, &lease.key)?.ok_or_else(|| Error::upstream("busy", "公共编辑输入写入租约已变更"))
}
fn material_value(m: &Material) -> Value {
    let mut value = json!({"title":m.title,"url":m.url,"text":crate::clip(&m.text,18000),"published":m.published,
        "discovered":m.discovered,"discoveredVia":m.discovered_via,"publicationSource":m.publication_source,
        "publisher":m.publisher,"fulltext":m.fulltext,"extraction":m.extraction()});
    if m.pdf.is_some() && m.text.chars().count() > 18000 {
        value["extraction"]["truncated"] = json!(true);
    }
    value
}
fn restore_material(v: &Value) -> Result<Material> {
    let required = |key: &str| {
        v[key]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(Error::internal)
    };
    let optional = |key: &str| v[key].as_str().map(str::to_owned);
    let publication_source = match v["publicationSource"].as_str().unwrap_or_default() {
        "original_page_metadata" => "original_page_metadata",
        "feed_publication" => "feed_publication",
        "search_reported_publication" => "search_reported_publication",
        "crossref_publication" => "crossref_publication",
        _ => "unknown",
    };
    Ok(Material {
        title: required("title")?,
        url: required("url")?,
        text: required("text")?,
        published: optional("published"),
        discovered: optional("discovered"),
        discovered_via: optional("discoveredVia"),
        publication_source,
        publisher: required("publisher")?,
        fulltext: v["fulltext"] == true,
        pdf: if v["extraction"].is_null() {
            None
        } else {
            Some(
                crate::discovery::PdfExtraction::from_value(&v["extraction"])
                    .ok_or_else(Error::internal)?,
            )
        },
    })
}
fn ensure_new_sources(
    app: &App,
    topic: &str,
    language: &str,
    materials: &[Material],
    gaps: &[String],
) -> Result<()> {
    let Some(previous) = super::previous_edition(app, topic, language)? else {
        return Ok(());
    };
    // An explicit pipeline revision gets one freshly frozen edition. Later windows
    // reuse the same-version source set without repeated paid re-editing.
    if previous["pipelineVersion"]
        .as_str()
        .is_some_and(|v| v != PIPELINE_VERSION)
    {
        return Ok(());
    }
    let mut known = std::collections::HashSet::new();
    for url in previous["candidateUrls"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        known.insert(crate::freshness::canonical(url));
    }
    for source in previous["sources"].as_array().into_iter().flatten() {
        if let Some(url) = source["url"].as_str() {
            known.insert(crate::freshness::canonical(url));
        }
    }
    // Include the prior reserve, not only sources chosen by the editor: an unchanged
    // unselected candidate must not make every subsequent window look new.
    if let Some(snapshot) = previous["id"]
        .as_str()
        .and_then(|id| load(app, id).ok().flatten())
    {
        for source in snapshot["materials"].as_array().into_iter().flatten() {
            if let Some(url) = source["url"].as_str() {
                known.insert(crate::freshness::canonical(url));
            }
        }
    }
    if !materials.is_empty()
        && materials
            .iter()
            .all(|m| known.contains(&crate::freshness::canonical(&m.url)))
    {
        return Err(Error::upstream(
            if gaps.is_empty() {
                "no_new_sources"
            } else {
                "public_sources_incomplete"
            },
            if gaps.is_empty() {
                "本时段未取得新原文，保留上一版与原日期"
            } else {
                "来源覆盖存在缺口，尚未核验到新增资料，保留上一版与原日期"
            },
        ));
    }
    Ok(())
}
pub(super) async fn build(
    app: &App,
    topic: &'static CatalogTopic,
    language: &str,
    lease: &Lease,
    date: &str,
) -> Result<Value> {
    let snapshot = if let Some(snapshot) = load(app, &lease.key)? {
        snapshot
    } else {
        let retrieved = retrieval::collect(app, topic).await?;
        ensure_new_sources(
            app,
            topic.id,
            language,
            &retrieved.materials,
            &retrieved.gaps,
        )?;
        let materials: Vec<_> = retrieved.materials.iter().map(material_value).collect();
        freeze(
            app,
            lease,
            &json!({"materials":materials,"metadata":retrieved.metadata,"gaps":retrieved.gaps,"aihotContext":retrieved.aihot_context,"createdAt":now()}),
        )?
    };
    let materials: Vec<_> = snapshot["materials"]
        .as_array()
        .ok_or_else(Error::internal)?
        .iter()
        .map(restore_material)
        .collect::<Result<_>>()?;
    let mut value = crate::editorial::compose(
        app,
        &lease.key,
        topic,
        &materials,
        snapshot["aihotContext"].clone(),
    )
    .await?;
    if let Some(sources) = value["sources"].as_array_mut() {
        for source in sources {
            let url = source["url"].as_str().unwrap_or_default().to_owned();
            let meta = &snapshot["metadata"][&url];
            // Sources discovered later by the agent already carry their own retrieval time.
            if let Some(at) = meta[0]
                .as_i64()
                .and_then(|at| chrono::DateTime::from_timestamp(at, 0))
            {
                source["retrievedAt"] = json!(at.to_rfc3339());
                source["cacheStatus"] = meta[1].clone();
            }
            source["sourceLayer"] = json!(source["discoveredVia"]
                .as_str()
                .and_then(catalog_data::policy)
                .map(|p| p.layer)
                .unwrap_or("公开原文"));
        }
    }
    value["id"] = json!(lease.key);
    value["catalogTopicId"] = json!(topic.id);
    value["utcDate"] = json!(date);
    value["language"] = json!(language);
    value["pipelineVersion"] = json!(PIPELINE_VERSION);
    value["createdAt"] = snapshot["createdAt"].clone();
    // Public URL metadata outlives the 24h raw-text snapshot; reserve candidates
    // must not become new again merely because their unselected text was erased.
    value["candidateUrls"] = json!(materials
        .iter()
        .map(|m| crate::freshness::canonical(&m.url))
        .collect::<std::collections::BTreeSet<_>>());
    let mut gaps = value["gaps"].as_array().cloned().unwrap_or_default();
    gaps.extend(snapshot["gaps"].as_array().cloned().unwrap_or_default());
    value["gaps"] = json!(gaps);
    Ok(value)
}
pub(super) fn retention(conn: &Connection, at: i64) -> Result<()> {
    // Keep the key/fingerprint tombstone: clearing source text must never trigger a fresh paid call.
    conn.execute(
        "UPDATE public_editorial_inputs SET body=NULL WHERE expires<=? AND body IS NOT NULL",
        [at],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clipped_pdf_snapshot_keeps_a_conservative_read_boundary() {
        let value = json!({"title":"Report","url":"https://example.org/report.pdf",
            "text":"x".repeat(20000),"publisher":"Example","fulltext":true,
            "extraction":{"format":"pdf","pagesRead":3,"totalPages":3,"truncated":false}});
        let original = restore_material(&value).unwrap();
        let restored = restore_material(&material_value(&original)).unwrap();
        assert_eq!(restored.text.chars().count(), 18000);
        assert!(restored.pdf.unwrap().truncated);
        assert!(!original.pdf.unwrap().truncated);
    }
    #[test]
    fn retry_uses_exact_frozen_input_and_expiry_cannot_recreate_it() {
        let app =
            App::open(std::env::temp_dir().join(format!("public-inputs-{}", crate::id()))).unwrap();
        let lease = match super::super::acquire(&app, "snapshot-test", "edition").unwrap() {
            super::super::Reservation::Refresh(l) => l,
            _ => panic!("lease"),
        };
        let first =
            json!({"materials":[{"text":"public evidence version one"}],"metadata":{},"gaps":[]});
        assert_eq!(freeze(&app, &lease, &first).unwrap(), first);
        assert_eq!(
            freeze(&app, &lease, &json!({"materials":[{"text":"changed"}]})).unwrap(),
            first
        );
        app.store
            .conn()
            .unwrap()
            .execute("UPDATE public_editorial_inputs SET expires=0", [])
            .unwrap();
        retention(&app.store.conn().unwrap(), Utc::now().timestamp()).unwrap();
        assert_eq!(
            load(&app, &lease.key).unwrap_err().code,
            "public_inputs_expired"
        );
        assert_eq!(
            freeze(&app, &lease, &first).unwrap_err().code,
            "public_inputs_expired"
        );
        let _ = std::fs::remove_dir_all(app.data_dir.as_ref());
    }
    #[test]
    fn stale_retrieval_cannot_freeze_after_lease_takeover() {
        let app =
            App::open(std::env::temp_dir().join(format!("public-fence-{}", crate::id()))).unwrap();
        let lease = match super::super::acquire(&app, "snapshot-test", "edition").unwrap() {
            super::super::Reservation::Refresh(l) => l,
            _ => panic!("lease"),
        };
        app.store
            .conn()
            .unwrap()
            .execute("UPDATE public_cache SET lease_token='new-owner'", [])
            .unwrap();
        assert!(freeze(&app, &lease, &json!({"materials":[]})).is_err());
        assert!(load(&app, &lease.key).unwrap().is_none());
        let _ = std::fs::remove_dir_all(app.data_dir.as_ref());
    }

    #[test]
    fn unchanged_original_urls_do_not_enter_another_paid_editing_window() {
        let app =
            App::open(std::env::temp_dir().join(format!("public-no-new-{}", crate::id()))).unwrap();
        let at = Utc::now().timestamp();
        let prior = json!({"id":"prior","catalogTopicId":"ai","language":"zh-CN","pipelineVersion":PIPELINE_VERSION,"utcDate":"2026-10-12","sources":[{"url":"https://example.org/accepted"}]});
        app.store.conn().unwrap().execute("INSERT INTO public_cache(cache_key,kind,body,retain_until) VALUES('prior','edition',?,?)",params![prior.to_string(),at+86400]).unwrap();
        let snapshot = json!({"materials":[{"url":"https://example.org/reserve"}]}).to_string();
        app.store
            .conn()
            .unwrap()
            .execute(
                "INSERT INTO public_editorial_inputs VALUES('prior',?,?,?,?)",
                params![snapshot, hash(snapshot.as_bytes()), at, at + 86400],
            )
            .unwrap();
        let make = |url: &str| Material {
            title: "test".into(),
            url: url.into(),
            text: "verified original".into(),
            published: Some(now()),
            discovered: None,
            discovered_via: None,
            publication_source: "original_page_metadata",
            publisher: "example.org".into(),
            fulltext: true,
            pdf: None,
        };
        assert_eq!(
            ensure_new_sources(
                &app,
                "ai",
                "zh-CN",
                &[
                    make("https://example.org/accepted?utm_source=rss"),
                    make("https://example.org/reserve#top")
                ],
                &[],
            )
            .unwrap_err()
            .code,
            "no_new_sources"
        );
        assert!(
            ensure_new_sources(&app, "ai", "zh-CN", &[make("https://example.org/new")], &[])
                .is_ok()
        );
        app.store.conn().unwrap().execute("UPDATE public_cache SET body=json_set(body,'$.candidateUrls',json(?)) WHERE cache_key='prior'",[json!(["https://example.org/reserve"]).to_string()]).unwrap();
        app.store
            .conn()
            .unwrap()
            .execute(
                "UPDATE public_editorial_inputs SET body=NULL,expires=0 WHERE edition_key='prior'",
                [],
            )
            .unwrap();
        assert_eq!(
            ensure_new_sources(
                &app,
                "ai",
                "zh-CN",
                &[
                    make("https://example.org/accepted"),
                    make("https://example.org/reserve")
                ],
                &[]
            )
            .unwrap_err()
            .code,
            "no_new_sources",
            "raw snapshot expiry must not make old unselected candidates new again"
        );
        assert_eq!(
            ensure_new_sources(
                &app,
                "ai",
                "zh-CN",
                &[make("https://example.org/accepted")],
                &["原网页读取超时".into()]
            )
            .unwrap_err()
            .code,
            "public_sources_incomplete"
        );
        assert_eq!(
            app.store
                .conn()
                .unwrap()
                .query_row("SELECT count(*) FROM public_model_calls", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        let _ = std::fs::remove_dir_all(app.data_dir.as_ref());
    }
    #[test]
    fn prior_pipeline_candidates_do_not_block_a_corrected_editorial_run() {
        let app = App::open(std::env::temp_dir().join(format!("public-upgrade-{}", crate::id())))
            .unwrap();
        let old = json!({"id":"prior-version","catalogTopicId":"ai","language":"zh-CN",
            "pipelineVersion":"retired-editorial-pipeline","utcDate":"2026-10-12",
            "candidateUrls":["https://example.org/accepted"],"sources":[{"url":"https://example.org/accepted"}]});
        app.store.conn().unwrap().execute("INSERT INTO public_cache(cache_key,kind,body,retain_until) VALUES('prior-version','edition',?,?)",params![old.to_string(),Utc::now().timestamp()+86400]).unwrap();
        let original = Material {
            title: "original".into(),
            url: "https://example.org/accepted".into(),
            text: "Already collected evidence can be re-edited by a new pipeline.".into(),
            published: Some(now()),
            discovered: None,
            discovered_via: None,
            publication_source: "original_page_metadata",
            publisher: "Example".into(),
            fulltext: true,
            pdf: None,
        };
        assert!(ensure_new_sources(&app, "ai", "zh-CN", &[original.clone()], &[]).is_ok());
        let mut current = old;
        current["pipelineVersion"] = json!(PIPELINE_VERSION);
        app.store
            .conn()
            .unwrap()
            .execute(
                "UPDATE public_cache SET body=? WHERE cache_key='prior-version'",
                [current.to_string()],
            )
            .unwrap();
        assert_eq!(
            ensure_new_sources(&app, "ai", "zh-CN", &[original], &[])
                .unwrap_err()
                .code,
            "no_new_sources"
        );
        assert_eq!(
            app.store
                .conn()
                .unwrap()
                .query_row("SELECT count(*) FROM public_model_calls", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        let _ = std::fs::remove_dir_all(app.data_dir.as_ref());
    }
    #[test]
    fn discovery_snapshot_preserves_unread_search_marker_and_unknown_date() {
        let candidate = Material {
            title: "Unverified discovery".into(),
            url: "https://example.org/candidate".into(),
            text: "A discovery snippet cannot support an editorial claim.".into(),
            published: None,
            discovered: Some(now()),
            discovered_via: Some("search".into()),
            publication_source: "search_reported_publication",
            publisher: "Example".into(),
            fulltext: false,
            pdf: None,
        };
        let restored = restore_material(&material_value(&candidate)).unwrap();
        assert!(restored.is_search_snippet());
        assert!(!restored.fulltext);
        assert!(restored.published.is_none());
        assert_eq!(restored.text, candidate.text);
    }
}
