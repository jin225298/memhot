mod common;
use axum::http::StatusCode;
use common::{request, user, Fixture};
use memhot_server::{hash, now, App};
use rusqlite::params;
use serde_json::{json, Value};

fn put(app: &App, user: &str, kind: &str, value: Value) {
    app.store.put(user, kind, &value, None).unwrap();
}
fn report() -> Value {
    json!({"id":"r1","kind":"research","title":"授权的报告","createdAt":now(),"body":"报告正文","status":"ready","sources":[{"id":"e1","title":"授权来源","url":"https://example.com/material","publisher":"Example","publishedAt":"2026-10-11T00:00:00Z","retrievalKind":"page","excerpt":"保留的报告摘录"}]})
}
fn raw(app: &App, user: &str, text: &str, expires: i64) {
    let url = "https://example.com/material";
    put(
        app,
        user,
        "raw",
        json!({"id":hash(url.as_bytes()),"url":url,"text":text,"expiresAt":expires,"createdAt":now()}),
    );
}
const MATERIAL: &str = "/api/v1/reports/r1/materials/e1";
const TRANSLATION: &str = "/api/v1/reports/r1/materials/e1/translation";

#[tokio::test]
async fn materials_require_owned_report_and_exact_evidence_and_label_expiry_and_truncation() {
    let f = Fixture::new();
    let app = &f.app;
    let (a, ta) = user(app, "A").await;
    let (_, tb) = user(app, "B").await;
    put(app, &a, "report", report());
    raw(
        app,
        &a,
        "当前保存的完整正文",
        chrono::Utc::now().timestamp() + 3600,
    );
    let (status, full) = request(app, "GET", MATERIAL, &ta, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(full["text"], "当前保存的完整正文");
    assert_eq!(full["availability"], "full");
    assert_eq!(full["retrievalKind"], "page");
    assert_eq!(full["truncated"], false);
    assert_eq!(full["publishedAt"], "2026-10-11T00:00:00Z");
    for (method, path) in [("GET", MATERIAL), ("POST", TRANSLATION)] {
        assert_eq!(
            request(app, method, path, &tb, None).await.0,
            StatusCode::NOT_FOUND
        );
    }
    assert_eq!(
        request(
            app,
            "GET",
            "/api/v1/reports/r1/materials/unknown",
            &ta,
            None
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    raw(
        app,
        &a,
        &"长".repeat(18000),
        chrono::Utc::now().timestamp() + 3600,
    );
    let bounded = request(app, "GET", MATERIAL, &ta, None).await.1;
    assert_eq!(bounded["text"].as_str().unwrap().chars().count(), 12000);
    assert_eq!(bounded["truncated"], true);
    assert!(bounded["message"].as_str().unwrap().contains("12000"));
    raw(
        app,
        &a,
        "expired-secret-original",
        chrono::Utc::now().timestamp() - 1,
    );
    let expired = request(app, "GET", MATERIAL, &ta, None).await.1;
    assert_eq!(expired["availability"], "excerpt");
    assert_eq!(expired["retrievalKind"], "metadata_or_abstract");
    assert_eq!(expired["text"], "保留的报告摘录");
    assert!(!expired.to_string().contains("expired-secret-original"));
    raw(
        app,
        &a,
        "too-old-even-with-extended-expiry",
        chrono::Utc::now().timestamp() + 3600,
    );
    app.store
        .conn()
        .unwrap()
        .execute(
            "UPDATE records SET data=json_set(data,'$.createdAt',?) WHERE user_id=? AND kind='raw'",
            params![
                (chrono::Utc::now() - chrono::Duration::hours(25)).to_rfc3339(),
                a
            ],
        )
        .unwrap();
    assert_eq!(
        request(app, "GET", MATERIAL, &ta, None).await.1["availability"],
        "excerpt"
    );
    raw(
        app,
        &a,
        "论文落地页摘要",
        chrono::Utc::now().timestamp() + 3600,
    );
    let mut abstract_report = report();
    abstract_report["sources"][0]["retrievalKind"] = json!("metadata_or_abstract");
    put(app, &a, "report", abstract_report);
    assert_eq!(
        request(app, "GET", MATERIAL, &ta, None).await.1["availability"],
        "excerpt"
    );
}

#[tokio::test]
async fn shared_material_pages_are_read_only_and_never_beyond_twenty_four_hours() {
    let f = Fixture::new();
    let app = &f.app;
    let (a, ta) = user(app, "A").await;
    let (_, tb) = user(app, "B").await;
    let at = chrono::Utc::now().timestamp();
    let mut edition = report();
    edition["body"] = json!("authorized-public-fulltext-needle");
    edition["editorial"] =
        json!({"stories":[{"id":"s1","title":"已核对的事件","evidenceIds":["e1"]}]});
    app.store.conn().unwrap().execute("INSERT INTO public_cache(cache_key,kind,body,expires_at,retain_until) VALUES('edition','edition',?,?,?)",params![edition.to_string(),at+3600,at+86400]).unwrap();
    put(
        app,
        &a,
        "report",
        json!({"id":"r1","kind":"digest","createdAt":now(),"publicEditionId":"edition"}),
    );
    assert_eq!(
        request(
            app,
            "GET",
            "/api/v1/reading?q=authorized-public-fulltext-needle",
            &ta,
            None
        )
        .await
        .1["overview"]["reportCount"],
        1
    );
    assert_eq!(
        request(
            app,
            "GET",
            "/api/v1/reading?q=authorized-public-fulltext-needle",
            &tb,
            None
        )
        .await
        .1["reports"],
        json!([])
    );
    let page = json!({"url":"https://example.com/material","contentType":"text/html","fetchedAt":at-3600,"text":"<html><body><article>保留的公共正文<script>do not execute</script></article></body></html>"});
    // Refresh TTL is only 30min. A retained, explicitly dated body may still be read for 24h.
    app.store.conn().unwrap().execute("INSERT INTO public_cache(cache_key,kind,body,expires_at,retain_until) VALUES('page-test','page',?,?,?)",params![page.to_string(),at-1,at+86400]).unwrap();
    let material = request(app, "GET", MATERIAL, &ta, None).await.1;
    assert_eq!(material["availability"], "full");
    assert_eq!(material["text"], "保留的公共正文");
    assert!(!material.to_string().contains("do not execute"));
    assert_eq!(
        request(app, "GET", MATERIAL, &tb, None).await.0,
        StatusCode::NOT_FOUND
    );
    app.store.conn().unwrap().execute("UPDATE public_cache SET body=json_set(body,'$.fetchedAt',?) WHERE cache_key='page-test'",[at-86401]).unwrap();
    let expired = request(app, "GET", MATERIAL, &ta, None).await.1;
    assert_eq!(expired["availability"], "excerpt");
    assert_eq!(expired["text"], "保留的报告摘录");
    app.store
        .conn()
        .unwrap()
        .execute("DELETE FROM public_cache WHERE kind='page'", [])
        .unwrap();
    assert_eq!(
        request(app, "GET", MATERIAL, &ta, None).await.1["text"],
        "保留的报告摘录"
    );
    let pages: i64 = app
        .store
        .conn()
        .unwrap()
        .query_row(
            "SELECT count(*) FROM public_cache WHERE kind='page'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(pages, 0, "reading must never cause a new network fetch");
}

#[tokio::test]
async fn translation_is_idempotent_and_its_public_status_does_not_echo_provider_errors() {
    let f = Fixture::new();
    let app = &f.app;
    let (a, ta) = user(app, "A").await;
    let (_, tb) = user(app, "B").await;
    put(app, &a, "report", report());
    raw(
        app,
        &a,
        "English source material",
        chrono::Utc::now().timestamp() + 3600,
    );
    let (status, first) = request(app, "POST", TRANSLATION, &ta, Some(json!({}))).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(first["status"], "queued");
    app.store
        .conn()
        .unwrap()
        .execute(
            "UPDATE billing_wallets SET included=0,topup=0 WHERE user_id=?",
            [&a],
        )
        .unwrap();
    let repeated = request(app, "POST", TRANSLATION, &ta, Some(json!({}))).await;
    assert_eq!(repeated.0, StatusCode::ACCEPTED);
    assert_eq!(repeated.1["id"], first["id"]);
    assert_eq!(app.store.jobs(&a).unwrap().len(), 1);
    let material = request(app, "GET", MATERIAL, &ta, None).await.1;
    assert_eq!(material["translation"]["status"], "queued");
    assert_eq!(material["translation"]["jobId"], first["id"]);
    assert_eq!(
        request(app, "POST", TRANSLATION, &tb, None).await.0,
        StatusCode::NOT_FOUND
    );
    app.store.conn().unwrap().execute("UPDATE jobs SET status='failed',message='upstream-secret-authenticated-body' WHERE id=?",[first["id"].as_str().unwrap()]).unwrap();
    let failed = request(app, "GET", MATERIAL, &ta, None).await.1;
    assert_eq!(failed["translation"]["status"], "failed");
    assert!(!failed.to_string().contains("upstream-secret"));
    let repeated = request(app, "POST", TRANSLATION, &ta, None).await.1;
    assert!(!repeated.to_string().contains("upstream-secret"));
    assert_eq!(repeated["id"], first["id"]);
    let mut changed = report();
    changed["sources"][0]["title"] = json!("修订后的来源标题，正文相同");
    put(app, &a, "report", changed);
    assert!(request(app, "GET", MATERIAL, &ta, None)
        .await
        .1
        .get("translation")
        .is_none());
    app.store
        .conn()
        .unwrap()
        .execute(
            "UPDATE billing_wallets SET included=10 WHERE user_id=?",
            [&a],
        )
        .unwrap();
    let new = request(app, "POST", TRANSLATION, &ta, None).await.1;
    assert_ne!(
        new["id"], first["id"],
        "changed evidence must not reuse the failed logical input"
    );
}

#[tokio::test]
async fn deleted_or_expired_context_stops_translation_before_any_sdk_charge() {
    let f = Fixture::new();
    let app = &f.app;
    let (a, ta) = user(app, "A").await;
    put(app, &a, "report", report());
    raw(
        app,
        &a,
        "English source material",
        chrono::Utc::now().timestamp() + 3600,
    );
    request(app, "POST", TRANSLATION, &ta, None).await;
    let job = app.store.claim().unwrap().unwrap();
    app.store
        .conn()
        .unwrap()
        .execute("UPDATE users SET epoch=epoch+1 WHERE id=?", [&a])
        .unwrap();
    assert_eq!(
        memhot_server::worker::execute(app, &job)
            .await
            .unwrap_err()
            .status,
        StatusCode::NOT_FOUND
    );
    assert!(app.store.list_all(&a, "agent_run").unwrap().is_empty());
    let billable: i64 = app
        .store
        .conn()
        .unwrap()
        .query_row(
            "SELECT count(*) FROM billing_usage WHERE user_id=?",
            [&a],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(billable, 0);
    app.store
        .fail(&job, &memhot_server::error::Error::missing())
        .unwrap();
    let queue = request(app, "POST", TRANSLATION, &ta, None).await.1;
    assert_ne!(queue["id"], job.id);
    let second = app.store.claim().unwrap().unwrap();
    raw(
        app,
        &a,
        "expired original",
        chrono::Utc::now().timestamp() - 1,
    );
    assert_eq!(
        memhot_server::worker::execute(app, &second)
            .await
            .unwrap_err()
            .code,
        "translation_context_changed"
    );
    assert!(app.store.list_all(&a, "agent_run").unwrap().is_empty());
}

#[tokio::test]
async fn absent_material_text_has_an_actionable_translation_error() {
    let f = Fixture::new();
    let app = &f.app;
    let (a, ta) = user(app, "A").await;
    let mut value = report();
    value["sources"][0]["excerpt"] = json!("");
    put(app, &a, "report", value);
    let (status, value) = request(app, "POST", TRANSLATION, &ta, None).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(value["error"]["code"], "material_text_unavailable");
    assert!(app.store.jobs(&a).unwrap().is_empty());
}

#[tokio::test]
async fn concurrent_translation_posts_share_one_job_and_manual_retry_state_follows_the_child() {
    let f = Fixture::new();
    let app = &f.app;
    let (a, ta) = user(app, "A").await;
    put(app, &a, "report", report());
    raw(
        app,
        &a,
        "Original text",
        chrono::Utc::now().timestamp() + 3600,
    );
    let (first, second) = tokio::join!(
        request(app, "POST", TRANSLATION, &ta, None),
        request(app, "POST", TRANSLATION, &ta, None)
    );
    assert_eq!(first.0, StatusCode::ACCEPTED);
    assert_eq!(first.1["id"], second.1["id"]);
    let id = first.1["id"].as_str().unwrap();
    app.store
        .conn()
        .unwrap()
        .execute("UPDATE jobs SET status='failed' WHERE id=?", [id])
        .unwrap();
    let (status, retry) = request(
        app,
        "POST",
        &format!("/api/v1/jobs/{id}/retry"),
        &ta,
        Some(json!({"acceptDuplicateCharge":false})),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_ne!(retry["id"], id);
    let material = request(app, "GET", MATERIAL, &ta, None).await.1;
    assert_eq!(material["translation"]["status"], "queued");
    assert_eq!(material["translation"]["jobId"], retry["id"]);
}
