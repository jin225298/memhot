mod common;
use common::*;
use memhot_server::{profile, sources, worker, App};
use serde_json::{json, Value};
#[tokio::test]
async fn private_publication_checks_epoch_atomically_and_strips_internal_stamp() {
    let f = Fixture::new();
    let (uid, _) = user(&f.app, "publication").await;
    let epoch = f.app.store.epoch(&uid).unwrap();
    f.app
        .store
        .queue(&uid, "digest", json!({}), "epoch-check")
        .unwrap();
    let job = f.app.store.claim().unwrap().unwrap();
    let report = json!({"id":"private", "_userEpoch":epoch, "createdAt":memhot_server::now()});
    f.app
        .store
        .conn()
        .unwrap()
        .execute("UPDATE users SET epoch=epoch+1 WHERE id=?", [&uid])
        .unwrap();
    assert!(!f
        .app
        .store
        .finish(&job, &[("report", report)], "late")
        .unwrap());
    assert!(f.app.store.list(&uid, "report").unwrap().is_empty());
    let fresh = json!({"id":"private", "_userEpoch":epoch+1, "createdAt":memhot_server::now()});
    assert!(f
        .app
        .store
        .finish(&job, &[("report", fresh)], "current")
        .unwrap());
    assert!(f
        .app
        .store
        .get(&uid, "report", "private")
        .unwrap()
        .get("_userEpoch")
        .is_none());
}
#[tokio::test]
async fn deleting_a_preference_revokes_private_agent_context_for_only_its_owner() {
    let f = Fixture::new();
    let (uid, token) = user(&f.app, "owner").await;
    let (other, _) = user(&f.app, "other").await;
    let (status, preference) = request(
        &f.app,
        "POST",
        "/api/v1/cognition",
        &token,
        Some(json!({"type":"preference","statement":"先阅读一手资料"})),
    )
    .await;
    assert_eq!(status, 200);
    for user in [&uid, &other] {
        for kind in ["agent_run", "discussion_request"] {
            f.app
                .store
                .put(
                    user,
                    kind,
                    &json!({"id":"continuation","private":"context"}),
                    None,
                )
                .unwrap();
        }
    }
    let epoch = f.app.store.epoch(&uid).unwrap();
    let other_epoch = f.app.store.epoch(&other).unwrap();
    let scope = memhot_server::agent::Scope::private(&f.app, &uid, None, "pending").unwrap();
    assert_eq!(
        request(
            &f.app,
            "DELETE",
            &format!("/api/v1/cognition/{}", preference["id"].as_str().unwrap()),
            &token,
            None
        )
        .await
        .0,
        204
    );
    assert_eq!(f.app.store.epoch(&uid).unwrap(), epoch + 1);
    assert_eq!(f.app.store.epoch(&other).unwrap(), other_epoch);
    for kind in ["agent_run", "discussion_request"] {
        assert!(f.app.store.list(&uid, kind).unwrap().is_empty());
        assert_eq!(f.app.store.list(&other, kind).unwrap().len(), 1);
    }
    // Even a tool-free run must reject its revoked scope before resolving a provider.
    let outcome = memhot_server::agent::run(
        &f.app,
        &scope,
        memhot_server::agent::Spec {
            mode: "review",
            purpose: "review",
            instructions: "review",
            prompt: json!({}),
            max_steps: 1,
            max_output_tokens: 128,
            tools: false,
        },
        memhot_server::agent::ToolContext::default(),
    )
    .await;
    assert_eq!(outcome.err().unwrap().code, "context_deleted");
}
#[tokio::test]
async fn formation_commit_preserves_feedback_and_cannot_resurrect_deleted_sources() {
    let f = Fixture::new();
    let app = &f.app;
    let (uid, token) = user(app, "A").await;
    let source = note(app, &token, "one").await;
    let key = source["id"].as_str().unwrap();
    let mut stale = app.store.list(&uid, "thought").unwrap()[0].clone();
    let thought = stale["id"].as_str().unwrap().to_owned();
    let job = app.store.claim().unwrap().unwrap();
    request(
        app,
        "POST",
        &format!("/api/v1/thoughts/{thought}/feedback"),
        &token,
        Some(json!({"action":"keep","note":"保留我的备注"})),
    )
    .await;
    stale["title"] = json!("模型整理后");
    stale["status"] = json!("open");
    stale["_mustExist"] = json!(true);
    assert!(app
        .store
        .finish(&job, &[("thought", stale.clone())], "done")
        .unwrap());
    let current = app.store.get(&uid, "thought", &thought).unwrap();
    assert_eq!(current["status"], "kept");
    assert_eq!(current["feedbackNote"], "保留我的备注");
    assert_eq!(current["title"], "模型整理后");
    app.store
        .queue(&uid, "formation", json!({"sourceId":key}), "again")
        .unwrap();
    let late = app.store.claim().unwrap().unwrap();
    let epoch = app.store.epoch(&uid).unwrap();
    sources::delete_owned(app, &uid, key).unwrap();
    assert!(app.store.epoch(&uid).unwrap() > epoch);
    assert!(!app
        .store
        .finish(&late, &[("source", source), ("thought", stale)], "too late")
        .unwrap());
    assert!(app.store.list(&uid, "thought").unwrap().is_empty());
    assert!(app.store.list(&uid, "source").unwrap().is_empty());
}
#[tokio::test]
async fn retention_uses_real_dates_and_deletes_dependent_personalization() {
    let f = Fixture::new();
    let app = &f.app;
    let (uid, token) = user(app, "A").await;
    let source = note(app, &token, "old").await;
    let key = source["id"].as_str().unwrap();
    let created = (chrono::Utc::now() - chrono::Duration::hours(25)).to_rfc3339();
    app.store
        .conn()
        .unwrap()
        .execute(
            "UPDATE records SET created=? WHERE user_id=? AND kind='source' AND id=?",
            rusqlite::params![created, uid, key],
        )
        .unwrap();
    let mut p = profile::default_profile();
    p["retentionDays"] = json!(1);
    app.store.save_profile(&uid, &p).unwrap();
    app.store.conn().unwrap().execute("INSERT INTO receipts(user_id,job_id,key,status,response,created) VALUES(?, 'x','old-receipt','completed','private raw',?)",rusqlite::params![uid,chrono::Utc::now().timestamp()-86401]).unwrap();
    worker::retention(app).unwrap();
    assert!(app.store.list(&uid, "source").unwrap().is_empty());
    assert!(app.store.list(&uid, "thought").unwrap().is_empty());
    assert!(app.store.jobs(&uid).unwrap().is_empty());
    let leaked: i64 = app
        .store
        .conn()
        .unwrap()
        .query_row(
            "SELECT count(*) FROM receipts WHERE response IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(leaked, 0);
}
#[tokio::test]
async fn leases_recover_and_stale_attempts_do_not_commit() {
    let f = Fixture::new();
    let app = &f.app;
    let (_uid, token) = user(app, "A").await;
    note(app, &token, "one").await;
    let first = app.store.claim().unwrap().unwrap();
    app.store
        .conn()
        .unwrap()
        .execute("UPDATE jobs SET lease_until=0 WHERE id=?", [&first.id])
        .unwrap();
    let reopened = App::open(app.data_dir.as_ref().clone()).unwrap();
    let second = reopened.store.claim().unwrap().unwrap();
    assert_eq!(second.attempt, first.attempt + 1);
    assert_eq!(second.id, first.id);
    assert!(!app.store.finish(&first, &[], "stale").unwrap());
    assert!(reopened.store.finish(&second, &[], "current").unwrap());
}
#[tokio::test]
async fn scheduler_separates_digest_skips_historical_threads_and_deduplicates() {
    let f = Fixture::new();
    let app = &f.app;
    let (uid, _) = user(app, "A").await;
    let mut p = profile::default_profile();
    p["digestHour"] = json!(0);
    p["interests"] = json!(["天文学"]);
    app.store.save_profile(&uid, &p).unwrap();
    for n in 0..5 {
        app.store.put(&uid,"thought",&json!({"id":format!("t{n}"),"createdAt":format!("2026-01-0{}T00:00:00Z",n+1),"status":"open","question":"problem","researchEligible":true}),None).unwrap();
    }
    worker::schedule(app).unwrap();
    worker::schedule(app).unwrap();
    let jobs = app.store.jobs(&uid).unwrap();
    assert_eq!(jobs.iter().filter(|j| j["kind"] == "research").count(), 0);
    assert_eq!(jobs.iter().filter(|j| j["kind"] == "digest").count(), 1);
    let payload: String = app
        .store
        .conn()
        .unwrap()
        .query_row(
            "SELECT payload FROM jobs WHERE user_id=? AND kind='digest'",
            [uid],
            |r| r.get(0),
        )
        .unwrap();
    assert!(serde_json::from_str::<Value>(&payload).unwrap()["thoughtId"].is_null());
}

#[tokio::test]
async fn automatic_research_waits_for_the_next_local_day_but_manual_research_does_not() {
    use chrono::TimeZone;
    let f = Fixture::new();
    let app = &f.app;
    for tz in [chrono_tz::Asia::Tokyo, chrono_tz::America::Los_Angeles] {
        let (uid, token) = user(app, tz.name()).await;
        let mut p = profile::default_profile();
        p["timezone"] = json!(tz.name());
        p["digestHour"] = json!(0);
        app.store.save_profile(&uid, &p).unwrap();
        let today = chrono::Utc::now().with_timezone(&tz).date_naive();
        let at = |day: chrono::NaiveDate, hour, minute, second| {
            tz.from_local_datetime(&day.and_hms_opt(hour, minute, second).unwrap())
                .single()
                .unwrap()
                .with_timezone(&chrono::Utc)
                .to_rfc3339()
        };
        for (key, created) in [
            (
                "yesterday",
                json!(at(today.pred_opt().unwrap(), 23, 59, 59)),
            ),
            ("today", json!(at(today, 0, 0, 0))),
            ("future", json!(at(today.succ_opt().unwrap(), 0, 0, 0))),
            ("invalid", json!("unknown")),
            ("missing", Value::Null),
        ] {
            app.store.put(&uid, "thought", &json!({"id":key,"createdAt":created,"status":"open","question":"problem","researchEligible":true}), None).unwrap();
        }
        let midday = tz
            .from_local_datetime(&today.and_hms_opt(12, 0, 0).unwrap())
            .single()
            .unwrap()
            .with_timezone(&chrono::Utc);
        worker::schedule_at(app, midday).unwrap();
        worker::schedule_at(app, midday).unwrap();
        let auto: Vec<String> = {
            let conn = app.store.conn().unwrap();
            let mut stmt = conn.prepare("SELECT json_extract(payload,'$.thoughtId') FROM jobs WHERE user_id=? AND kind='research'").unwrap();
            stmt.query_map([&uid], |r| r.get(0))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap()
        };
        assert_eq!(
            auto,
            ["yesterday"],
            "automatic research must wait until tomorrow in {tz}"
        );
        let manual = request(
            app,
            "POST",
            "/api/v1/research",
            &token,
            Some(json!({"thoughtId":"today"})),
        )
        .await;
        assert_eq!(manual.0, 202, "manual research may start today");
        let payload: String = app
            .store
            .conn()
            .unwrap()
            .query_row(
                "SELECT payload FROM jobs WHERE user_id=? AND id=?",
                rusqlite::params![uid, manual.1["id"].as_str().unwrap()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&payload).unwrap()["thoughtId"],
            "today"
        );
    }
}

#[tokio::test]
async fn missing_asr_keeps_audio_on_device_and_account_clear_fences_inflight_jobs() {
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;
    let f = Fixture::new();
    let app = &f.app;
    let (uid, token) = user(app, "A").await;
    let data = memhot_server::audio::wav(
        &vec![16; 320],
        asr_service::AudioFormat {
            sample_rate: 16000,
            channels: 1,
            bits_per_sample: 16,
        },
    );
    let req = Request::post("/api/v1/audio")
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "audio/wav")
        .header("idempotency-key", "before-config")
        .body(Body::from(data))
        .unwrap();
    let response = memhot_server::api::router(app.clone())
        .oneshot(req)
        .await
        .unwrap();
    assert_eq!(response.status(), 409);
    let count: i64 = app
        .store
        .conn()
        .unwrap()
        .query_row("SELECT count(*) FROM receipts", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        count, 0,
        "missing credentials must not reserve a paid request"
    );
    assert_eq!(
        std::fs::read_dir(app.data_dir.join("audio"))
            .map(|d| d.count())
            .unwrap_or(0),
        0
    );
    let source = app.store.list(&uid, "source").unwrap().remove(0);
    assert_eq!(source["status"], "awaiting_audio");
    assert!(source["_audioPath"].is_null());
    let note = note(app, &token, "clear-race").await;
    let running = app.store.claim().unwrap().unwrap();
    let epoch = app.store.epoch(&uid).unwrap();
    assert_eq!(
        request(app, "DELETE", "/api/v1/account/data", &token, None)
            .await
            .0,
        204
    );
    assert!(app.store.epoch(&uid).unwrap() > epoch);
    assert!(app.store.list(&uid, "source").unwrap().is_empty());
    assert!(app.store.jobs(&uid).unwrap().is_empty());
    assert!(!app.store.finish(&running, &[("source", json!({"id":note["id"],"status":"ready","text":"late transcript","createdAt":memhot_server::now()}))], "late").unwrap());
    assert_eq!(
        request(app, "GET", "/api/v1/home", &token, None).await.0,
        200,
        "clear keeps device paired"
    );
}
#[tokio::test]
async fn formation_cannot_recreate_a_deleted_neighbor_thread() {
    let f = Fixture::new();
    let app = &f.app;
    let (uid, token) = user(app, "A").await;
    let first = note(app, &token, "neighbor").await;
    let old_thread = app.store.list(&uid, "thought").unwrap()[0].clone();
    let initial = app.store.claim().unwrap().unwrap();
    app.store.finish(&initial, &[], "done").unwrap();
    let second = note(app, &token, "current").await;
    let job = app.store.claim().unwrap().unwrap();
    let mut merged = old_thread;
    merged["sourceIds"] = json!([first["id"], second["id"]]);
    merged["_mustExist"] = json!(true);
    sources::delete_owned(app, &uid, first["id"].as_str().unwrap()).unwrap();
    assert!(!app
        .store
        .finish(&job, &[("thought", merged)], "late merge")
        .unwrap());
    assert_eq!(app.store.list(&uid, "source").unwrap().len(), 1);
    assert_eq!(
        app.store.list(&uid, "thought").unwrap().len(),
        1,
        "only the second explicit note remains"
    );
}
