mod common;
use axum::{body::Body, http::Request};
use chrono::Timelike;
use common::*;
use http_body_util::BodyExt;
use memhot_server::{api, digest, hash, now, report_access, shared_public, worker, App};
use serde_json::{json, Value};
use tower::ServiceExt;

async fn refresh(app: &App, token: &str, request_id: &str) -> Value {
    let response = api::router(app.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/research")
                .header("authorization", format!("Bearer {token}"))
                .header("idempotency-key", request_id)
                .header("content-type", "application/json")
                .body(Body::from(json!({"kind":"digest"}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 202);
    serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap()
}

#[tokio::test]
async fn scheduled_digest_children_run_before_the_same_users_old_research_backlog() {
    let f = Fixture::new();
    let (uid, _) = user(&f.app, "A").await;
    let mut profile = f.app.store.profile(&uid).unwrap();
    profile["interests"] = json!(["AI"]);
    f.app.store.save_profile(&uid, &profile).unwrap();
    for index in 0..55 {
        f.app
            .store
            .queue(
                &uid,
                "research",
                json!({"kind":"research"}),
                &format!("old-{index}"),
            )
            .unwrap();
    }
    f.app
        .store
        .conn()
        .unwrap()
        .execute("UPDATE jobs SET created='2026-01-01T00:00:00Z'", [])
        .unwrap();
    f.app.store.queue(&uid, "digest", json!({"kind":"digest","researchDate":"2026-10-12","availableAt":"2026-10-13T00:00:00Z"}), "night-parent").unwrap();
    let parent = f.app.store.claim().unwrap().unwrap();
    assert_eq!(
        parent.kind, "digest",
        "today's scheduled edition cannot wait behind all historical research"
    );
    worker::execute(&f.app, &parent).await.unwrap();
    let child = f.app.store.claim().unwrap().unwrap();
    assert_eq!(child.kind, "digest");
    assert_eq!(child.payload["digestStage"], "public");
    assert_eq!(child.payload["availableAt"], "2026-10-13T00:00:00Z");
    assert!(
        f.app.store.claim().unwrap().is_none(),
        "one tenant still has only one running job"
    );
}

#[tokio::test]
async fn scheduled_digest_priority_preserves_tenant_rotation_and_expired_running_recovery() {
    let f = Fixture::new();
    let (a, _) = user(&f.app, "A").await;
    let (b, _) = user(&f.app, "B").await;
    f.app
        .store
        .queue(&a, "research", json!({}), "a-recovery")
        .unwrap();
    let old = f.app.store.claim().unwrap().unwrap();
    f.app
        .store
        .conn()
        .unwrap()
        .execute("UPDATE jobs SET lease_until=0 WHERE id=?", [&old.id])
        .unwrap();
    f.app
        .store
        .queue(
            &a,
            "digest",
            json!({"researchDate":"2026-10-12"}),
            "a-digest",
        )
        .unwrap();
    f.app
        .store
        .queue(&b, "research", json!({}), "b-research")
        .unwrap();
    let other = f.app.store.claim().unwrap().unwrap();
    assert_eq!(
        other.user, b,
        "a public edition cannot bypass another tenant's turn"
    );
    assert!(f.app.store.finish(&other, &[], "done").unwrap());
    let recovered = f.app.store.claim().unwrap().unwrap();
    assert_eq!(
        recovered.id, old.id,
        "recover unfinished work before starting another paid operation"
    );
    assert_eq!(recovered.attempt, 2);
    assert!(f.app.store.finish(&recovered, &[], &now()).unwrap());
    assert_eq!(f.app.store.claim().unwrap().unwrap().kind, "digest");
}

#[tokio::test]
async fn manual_digest_refresh_joins_queued_and_running_editions_and_publishes_without_waiting() {
    for running in [false, true] {
        let f = Fixture::new();
        let (uid, token) = user(&f.app, "A").await;
        let future = (chrono::Utc::now() + chrono::Duration::hours(8)).to_rfc3339();
        let scheduled = f.app.store.queue(&uid, "digest", json!({"kind":"digest","digestStage":"public","catalogTopicId":"ai","availableAt":future,"researchDate":"2026-10-12"}), "scheduled").unwrap();
        let mut job = running.then(|| f.app.store.claim().unwrap().unwrap());
        let response = refresh(&f.app, &token, "manual").await;
        assert_eq!(response["id"], scheduled["id"]);
        assert_eq!(refresh(&f.app, &token, "manual").await, response);
        if job.is_none() {
            job = f.app.store.claim().unwrap();
        }
        let stale_output = json!({"id":"report","kind":"digest","createdAt":now(),"availableAt":future,"body":"verified public result","sources":[]});
        f.app
            .store
            .finish(&job.unwrap(), &[("report", stale_output)], "done")
            .unwrap();
        let visible = report_access::get(&f.app, &uid, "report");
        assert!(
            visible.is_ok(),
            "joining a scheduled run must reveal its output immediately"
        );
        assert!(visible.unwrap().get("availableAt").is_none());
        let conn = f.app.store.conn().unwrap();
        assert_eq!(
            conn.query_row("SELECT count(*) FROM jobs", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            conn.query_row("SELECT count(*) FROM billing_usage", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            conn.query_row("SELECT count(*) FROM public_model_calls", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
}

#[tokio::test]
async fn manual_refresh_during_parent_dispatch_marks_all_children_for_immediate_delivery() {
    let f = Fixture::new();
    let (uid, token) = user(&f.app, "A").await;
    let mut profile = f.app.store.profile(&uid).unwrap();
    profile["interests"] = json!(["AI", "机器人"]);
    f.app.store.save_profile(&uid, &profile).unwrap();
    f.app.store.queue(&uid, "digest", json!({"kind":"digest","availableAt":"2099-01-01T00:00:00Z","researchDate":"2026-10-12"}), "parent").unwrap();
    let parent = f.app.store.claim().unwrap().unwrap();
    refresh(&f.app, &token, "manual-parent").await;
    worker::execute(&f.app, &parent).await.unwrap();
    let conn = f.app.store.conn().unwrap();
    let mut stmt = conn
        .prepare("SELECT payload FROM jobs WHERE status='queued'")
        .unwrap();
    let children: Vec<Value> = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .map(|r| serde_json::from_str(&r.unwrap()).unwrap())
        .collect();
    assert_eq!(children.len(), 2);
    for child in children {
        assert!(child.get("availableAt").is_none());
        assert_eq!(child["deliverImmediately"], true);
    }
}

#[tokio::test]
async fn stale_public_fallback_keeps_its_actual_edition_date_when_first_delivered() {
    let f = Fixture::new();
    let (uid, _) = user(&f.app, "A").await;
    let mut profile = f.app.store.profile(&uid).unwrap();
    profile["interests"] = json!(["AI"]);
    f.app.store.save_profile(&uid, &profile).unwrap();
    let at = chrono::Utc::now();
    let window = at
        .date_naive()
        .and_hms_opt(at.hour() / 6 * 6, 0, 0)
        .unwrap()
        .and_utc()
        .to_rfc3339();
    let current_key = format!(
        "edition-{}",
        hash(format!("ai:{window}:zh-CN:{}", shared_public::PIPELINE_VERSION).as_bytes())
    );
    let previous = json!({"id":"previous-edition","catalogTopicId":"ai","language":"zh-CN","pipelineVersion":shared_public::PIPELINE_VERSION,"utcDate":"2026-10-01","createdAt":"2026-10-01T10:00:00Z","gaps":[],"sources":[],"body":"old verified public edition"});
    f.app.store.conn().unwrap().execute("INSERT INTO public_cache(cache_key,kind,body,retain_until) VALUES('previous-edition','edition',?,?)",rusqlite::params![previous.to_string(),at.timestamp()+86400]).unwrap();
    f.app.store.conn().unwrap().execute("INSERT INTO public_cache(cache_key,kind,retry_after,last_error) VALUES(?,'edition',?,'network')",rusqlite::params![current_key,at.timestamp()+60]).unwrap();
    f.app
        .store
        .queue(
            &uid,
            "digest",
            json!({"kind":"digest","digestStage":"public","catalogTopicId":"ai"}),
            "deliver-old",
        )
        .unwrap();
    let job = f.app.store.claim().unwrap().unwrap();
    let outputs = digest::run(&f.app, &job).await.unwrap();
    f.app.store.finish(&job, &outputs, "done").unwrap();
    let key = outputs[0].1["id"].as_str().unwrap();
    let report = report_access::get(&f.app, &uid, key).unwrap();
    assert_eq!(
        report["createdAt"], "2026-10-01T10:00:00Z",
        "first delivery cannot redate stale content as a new edition"
    );
    assert_eq!(report["cacheStatus"], "stale");
}

#[tokio::test]
async fn first_manual_digest_refresh_runs_before_old_research_without_a_scheduled_parent() {
    let f = Fixture::new();
    let (uid, token) = user(&f.app, "A").await;
    for index in 0..55 {
        f.app
            .store
            .queue(&uid, "research", json!({}), &format!("old-{index}"))
            .unwrap();
    }
    f.app
        .store
        .conn()
        .unwrap()
        .execute("UPDATE jobs SET created='2026-01-01T00:00:00Z'", [])
        .unwrap();
    let response = refresh(&f.app, &token, "manual-first").await;
    let job = f.app.store.claim().unwrap().unwrap();
    assert_eq!(job.id, response["id"].as_str().unwrap());
    assert_eq!(job.kind, "digest");
}
