mod common;
use common::*;
use memhot_server::{daily, profile, worker, App};
use rusqlite::params;
use serde_json::{json, Value};

fn topic(app: &App, user: &str, key: &str, source: &str) -> Value {
    let value = json!({"id":key,"title":key,"question":format!("{key} 的问题"),"summary":format!("{key} 的讨论内容"),"sourceId":source,"sourceIds":[source],"evidenceRefs":[],"status":"open","researchEligible":true,"revision":1,"createdAt":"2026-10-12T10:00:00Z"});
    app.store.put(user, "thought", &value, None).unwrap();
    value
}
#[tokio::test]
async fn night_schedules_every_topic_beyond_home_page_once_with_morning_release() {
    let f = Fixture::new();
    let (uid, _) = user(&f.app, "A").await;
    let mut p = profile::default_profile();
    p["interests"] = json!(["天文"]);
    f.app.store.save_profile(&uid, &p).unwrap();
    for i in 0..75 {
        topic(&f.app, &uid, &format!("topic-{i}"), "s");
    }
    let at = chrono::DateTime::parse_from_rfc3339("2026-10-12T15:15:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    worker::schedule_at(&f.app, at).unwrap();
    worker::schedule_at(&f.app, at).unwrap();
    let conn = f.app.store.conn().unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT count(*) FROM jobs WHERE user_id=? AND kind='research'",
            [&uid],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        count, 75,
        "scheduler must not inherit a UI page or a three-topic cap"
    );
    let payload: String = conn
        .query_row(
            "SELECT payload FROM jobs WHERE user_id=? AND kind='research' LIMIT 1",
            [&uid],
            |r| r.get(0),
        )
        .unwrap();
    let p: Value = serde_json::from_str(&payload).unwrap();
    assert_eq!(p["researchDate"], "2026-10-12");
    assert_eq!(p["availableAt"], "2026-10-13T00:00:00+00:00");
}
#[tokio::test]
async fn users_run_concurrently_but_one_users_jobs_do_not_race_and_stale_failure_is_fenced() {
    let f = Fixture::new();
    let (a, _) = user(&f.app, "A").await;
    let (b, _) = user(&f.app, "B").await;
    f.app.store.queue(&a, "formation", json!({}), "a1").unwrap();
    f.app.store.queue(&a, "formation", json!({}), "a2").unwrap();
    f.app.store.queue(&b, "formation", json!({}), "b1").unwrap();
    let first = f.app.store.claim().unwrap().unwrap();
    let second = f.app.store.claim().unwrap().unwrap();
    assert_ne!(first.user, second.user);
    assert!(f.app.store.claim().unwrap().is_none());
    f.app
        .store
        .conn()
        .unwrap()
        .execute("UPDATE jobs SET lease_until=0 WHERE id=?", [&first.id])
        .unwrap();
    let renewed = f.app.store.claim().unwrap().unwrap();
    assert_eq!(renewed.id, first.id);
    f.app
        .store
        .fail(
            &first,
            &memhot_server::error::Error::upstream("timeout", "old attempt"),
        )
        .unwrap();
    let status: String = f
        .app
        .store
        .conn()
        .unwrap()
        .query_row("SELECT status FROM jobs WHERE id=?", [&renewed.id], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(status, "running");
}
#[tokio::test]
async fn manual_topic_edits_win_and_deleted_topic_cannot_be_recreated() {
    let f = Fixture::new();
    let (uid, token) = user(&f.app, "A").await;
    f.app
        .store
        .put(
            &uid,
            "source",
            &json!({"id":"s","text":"原文","status":"ready","createdAt":memhot_server::now()}),
            None,
        )
        .unwrap();
    let mut stale = topic(&f.app, &uid, "t", "s");
    f.app
        .store
        .queue(&uid, "formation", json!({"sourceId":"s"}), "form")
        .unwrap();
    let job = f.app.store.claim().unwrap().unwrap();
    let (status, edited) = request(
        &f.app,
        "PUT",
        "/api/v1/thoughts/t",
        &token,
        Some(json!({"title":"我的标题","question":"我的新问题","revision":1})),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(edited["revision"], 2);
    stale["title"] = json!("过期模型标题");
    stale["question"] = json!("过期模型问题");
    assert!(f
        .app
        .store
        .checkpoint(&job, &[("thought", stale.clone())])
        .unwrap());
    assert_eq!(
        f.app.store.get(&uid, "thought", "t").unwrap()["title"],
        "我的标题"
    );
    assert_eq!(
        request(&f.app, "DELETE", "/api/v1/thoughts/t", &token, None)
            .await
            .0,
        204
    );
    assert!(f
        .app
        .store
        .finish(&job, &[("thought", stale)], "done")
        .unwrap());
    assert!(f.app.store.get(&uid, "thought", "t").is_err());
}
#[tokio::test]
async fn cognition_is_editable_scoped_and_deleted_fingerprint_stays_deleted() {
    let f = Fixture::new();
    let (a, token) = user(&f.app, "A").await;
    let (_, other) = user(&f.app, "B").await;
    let (status, manual) = request(
        &f.app,
        "POST",
        "/api/v1/cognition",
        &token,
        Some(json!({"type":"preference","statement":"我偏好先看证据"})),
    )
    .await;
    assert_eq!(status, 200);
    let key = manual["id"].as_str().unwrap();
    assert_eq!(
        request(
            &f.app,
            "PUT",
            &format!("/api/v1/cognition/{key}"),
            &other,
            Some(json!({"statement":"覆盖"}))
        )
        .await
        .0,
        404
    );
    assert_eq!(
        request(
            &f.app,
            "PUT",
            &format!("/api/v1/cognition/{key}"),
            &token,
            Some(json!({"statement":"先看原始证据","revision":99}))
        )
        .await
        .0,
        409
    );
    assert_eq!(
        request(
            &f.app,
            "DELETE",
            &format!("/api/v1/cognition/{key}"),
            &token,
            None
        )
        .await
        .0,
        204
    );
    f.app
        .store
        .queue(&a, "cognition", json!({}), "inferred")
        .unwrap();
    let job = f.app.store.claim().unwrap().unwrap();
    let mut inferred = manual;
    inferred["id"] = json!("another-id");
    inferred["origin"] = json!("inferred");
    f.app
        .store
        .finish(&job, &[("cognition", inferred)], "done")
        .unwrap();
    assert!(f.app.store.list_all(&a, "cognition").unwrap().is_empty());
}
#[tokio::test]
async fn daily_sections_cover_multiple_topics_and_keep_partial_coverage_honest() {
    let f = Fixture::new();
    let (uid, _) = user(&f.app, "A").await;
    f.app.store.put(&uid,"source",&json!({"id":"s","text":"有两条话题","status":"ready","createdAt":"2026-10-12T10:00:00Z","analysisCoverage":{"includedChars":10000,"totalChars":45000,"complete":false}}),None).unwrap();
    topic(&f.app, &uid, "t1", "s");
    topic(&f.app, &uid, "t2", "s");
    f.app
        .store
        .queue(
            &uid,
            "daily_summary",
            json!({"localDate":"2026-10-12"}),
            "daily",
        )
        .unwrap();
    let job = f.app.store.claim().unwrap().unwrap();
    let result = daily::run(&f.app, &job).await.unwrap();
    let summary = &result[0].1;
    assert_eq!(summary["sections"].as_array().unwrap().len(), 2);
    assert_eq!(summary["status"], "partial");
    assert_eq!(summary["coverage"]["includedChars"], 10000);
    f.app.store.finish(&job, &result, "done").unwrap();
    memhot_server::sources::delete_owned(&f.app, &uid, "s").unwrap();
    assert!(f
        .app
        .store
        .list_all(&uid, "daily_summary")
        .unwrap()
        .is_empty());
    let count: i64 = f
        .app
        .store
        .conn()
        .unwrap()
        .query_row(
            "SELECT count(*) FROM records WHERE user_id=? AND kind='thought'",
            params![uid],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
}
