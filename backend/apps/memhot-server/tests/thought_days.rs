mod common;
use axum::http::StatusCode;
use common::*;
use memhot_server::{daily, worker};
use serde_json::{json, Value};

fn thought(f: &Fixture, uid: &str, id: &str, date: &str, status: &str) -> Value {
    let v = json!({"id":id,"title":id,"question":id,"summary":id,"status":status,"category":"产品","revision":1,"createdAt":format!("{date}T02:00:00Z"),"researchEligible":true});
    f.app.store.put(uid, "thought", &v, None).unwrap();
    v
}
fn today() -> String {
    chrono::Utc::now()
        .with_timezone(&chrono_tz::Asia::Shanghai)
        .date_naive()
        .to_string()
}
#[tokio::test]
async fn scopes_carry_identity_idempotency_and_tenant_isolation() {
    let f = Fixture::new();
    let (u, t) = user(&f.app, "A").await;
    let (_, other) = user(&f.app, "B").await;
    thought(&f, &u, "old", "2020-10-12", "open");
    thought(&f, &u, "new", &today(), "open");
    thought(&f, &u, "archived", &today(), "dismissed");
    let (s, today_list) = request(&f.app, "GET", "/api/v1/thoughts?scope=today", &t, None).await;
    assert_eq!(s, StatusCode::OK, "{today_list}");
    assert_eq!(today_list["total"], 1);
    assert_eq!(today_list["counts"]["history"], 2);
    assert_eq!(today_list["todayDate"], today());
    let (_, history) = request(&f.app, "GET", "/api/v1/thoughts?scope=history", &t, None).await;
    assert_eq!(history["total"], 2);
    assert_eq!(
        request(
            &f.app,
            "POST",
            "/api/v1/thoughts/old/today",
            &other,
            Some(json!({"revision":1}))
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        request(
            &f.app,
            "POST",
            "/api/v1/thoughts/old/today",
            &t,
            Some(json!({"revision":9}))
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let (s, carried) = request(
        &f.app,
        "POST",
        "/api/v1/thoughts/old/today",
        &t,
        Some(json!({"revision":1})),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{carried}");
    assert_eq!(carried["id"], "old");
    assert_eq!(carried["inToday"], true);
    assert_eq!(carried["thinkingDate"], today());
    let (_, again) = request(
        &f.app,
        "POST",
        "/api/v1/thoughts/old/today",
        &t,
        Some(json!({"revision":1})),
    )
    .await;
    assert_eq!(carried, again);
    let (_, history) = request(
        &f.app,
        "GET",
        "/api/v1/thoughts?scope=history&date=2020-10-12",
        &t,
        None,
    )
    .await;
    assert_eq!(history["items"][0]["id"], "old");
    assert_eq!(history["items"][0]["inToday"], true);
    assert_eq!(history["items"][0]["thinkingDate"], "2020-10-12");
    let (_, home) = request(&f.app, "GET", "/api/v1/home?thoughtScope=today", &t, None).await;
    assert_eq!(home["thoughts"].as_array().unwrap().len(), 2);
    let (_, detail) = request(&f.app, "GET", "/api/v1/thoughts/old", &t, None).await;
    assert_eq!(detail["inToday"], true);
    let conn = f.app.store.conn().unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT count(*) FROM records WHERE user_id=? AND kind='thought'",
            [&u],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 3);
    let paid: i64 = conn
        .query_row(
            "SELECT count(*) FROM jobs WHERE kind IN ('research','digest')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(paid, 0);
}
#[tokio::test]
async fn edit_does_not_carry_and_archived_carry_restores_once() {
    let f = Fixture::new();
    let (u, t) = user(&f.app, "A").await;
    thought(&f, &u, "old", "2020-10-12", "dismissed");
    let (s, _) = request(
        &f.app,
        "PUT",
        "/api/v1/thoughts/old",
        &t,
        Some(json!({"revision":1,"title":"改标题"})),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (_, list) = request(&f.app, "GET", "/api/v1/thoughts?scope=today", &t, None).await;
    assert_eq!(list["total"], 0);
    let (s, carried) = request(
        &f.app,
        "POST",
        "/api/v1/thoughts/old/today",
        &t,
        Some(json!({"revision":2})),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(carried["status"], "kept");
    assert_eq!(carried["revision"], 3);
    let (_, again) = request(
        &f.app,
        "POST",
        "/api/v1/thoughts/old/today",
        &t,
        Some(json!({"revision":2})),
    )
    .await;
    assert_eq!(again, carried);
    assert_eq!(
        request(
            &f.app,
            "POST",
            "/api/v1/thoughts/old/today",
            &t,
            Some(json!({"revision":99}))
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
}
#[tokio::test]
async fn late_formation_uses_source_day_and_model_merge_does_not_carry() {
    let f = Fixture::new();
    let (u, t) = user(&f.app, "A").await;
    let source = json!({"id":"s","occurredAt":"2020-10-11T23:00:00Z","createdAt":memhot_server::now(),"status":"ready","text":"原话"});
    f.app.store.put(&u, "source", &source, None).unwrap();
    let value = json!({"id":"formed","sourceIds":["s"],"title":"旧话题","question":"旧问题","status":"open","revision":1,"createdAt":memhot_server::now()});
    f.app
        .store
        .queue(&u, "formation", json!({"sourceId":"s"}), "form")
        .unwrap();
    let job = f.app.store.claim().unwrap().unwrap();
    assert!(f
        .app
        .store
        .checkpoint(&job, &[("thought", value.clone())])
        .unwrap());
    let mut changed = value;
    changed["summary"] = json!("模型更新");
    assert!(f
        .app
        .store
        .checkpoint(&job, &[("thought", changed)])
        .unwrap());
    let (_, list) = request(&f.app, "GET", "/api/v1/thoughts?scope=today", &t, None).await;
    assert_eq!(list["total"], 0);
    let (_, list) = request(&f.app, "GET", "/api/v1/thoughts?scope=history", &t, None).await;
    assert_eq!(list["items"][0]["thinkingDate"], "2020-10-12");
}
#[tokio::test]
async fn summary_carry_references_old_thought_without_same_day_source() {
    let f = Fixture::new();
    let (u, t) = user(&f.app, "A").await;
    thought(&f, &u, "old", "2020-10-12", "open");
    request(
        &f.app,
        "POST",
        "/api/v1/thoughts/old/today",
        &t,
        Some(json!({"revision":1})),
    )
    .await;
    let job = f.app.store.claim().unwrap().unwrap();
    let records = daily::run(&f.app, &job).await.unwrap();
    assert_eq!(records[0].1["sections"][0]["topicId"], "old");
    let conn = f.app.store.conn().unwrap();
    let n: i64 = conn
        .query_row(
            "SELECT count(*) FROM jobs WHERE json_extract(payload,'$.localDate')='2020-10-12'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n, 0);
}
#[tokio::test]
async fn scoped_cursor_binds_scope_timezone_and_membership_dates() {
    let f = Fixture::new();
    let (u, t) = user(&f.app, "A").await;
    for id in ["one", "two"] {
        thought(&f, &u, id, &today(), "open");
    }
    let (_, list) = request(
        &f.app,
        "GET",
        "/api/v1/thoughts?scope=today&limit=1",
        &t,
        None,
    )
    .await;
    let cursor = list["nextCursor"].as_str().unwrap();
    let (s, more) = request(
        &f.app,
        "GET",
        &format!("/api/v1/thoughts?scope=today&limit=1&before={cursor}"),
        &t,
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_ne!(list["items"][0]["id"], more["items"][0]["id"]);
    assert_eq!(
        request(
            &f.app,
            "GET",
            &format!("/api/v1/thoughts?scope=history&limit=1&before={cursor}"),
            &t,
            None
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let mut p = f.app.store.profile(&u).unwrap();
    p["timezone"] = json!("UTC");
    f.app.store.save_profile(&u, &p).unwrap();
    assert_eq!(
        request(
            &f.app,
            "GET",
            &format!("/api/v1/thoughts?scope=today&limit=1&before={cursor}"),
            &t,
            None
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
}
#[tokio::test]
async fn scheduler_ignores_old_backlog_and_stale_execution_keeps_receipts() {
    let f = Fixture::new();
    let (u, _) = user(&f.app, "A").await;
    thought(&f, &u, "old", "2020-10-12", "open");
    thought(&f, &u, "night", "2026-10-12", "open");
    let at = chrono::DateTime::parse_from_rfc3339("2026-10-12T15:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    worker::schedule_at(&f.app, at).unwrap();
    let conn = f.app.store.conn().unwrap();
    let n: i64 = conn
        .query_row("SELECT count(*) FROM jobs WHERE kind='research'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(n, 1);
    drop(conn);
    // Force an ancient auto job into running, with a potentially paid receipt.
    f.app
        .store
        .queue(
            &u,
            "research",
            json!({"thoughtId":"old","researchDate":"2020-10-12"}),
            "ancient",
        )
        .unwrap();
    let conn = f.app.store.conn().unwrap();
    conn.execute(
        "UPDATE jobs SET status='completed' WHERE dedup!='ancient'",
        [],
    )
    .unwrap();
    drop(conn);
    let job = f.app.store.claim().unwrap().unwrap();
    let conn = f.app.store.conn().unwrap();
    conn.execute(
        "INSERT INTO receipts(user_id,job_id,key,status,created) VALUES(?,?,?,'pending',0)",
        rusqlite::params![u, job.id, "paid"],
    )
    .unwrap();
    drop(conn);
    worker::execute(&f.app, &job).await.unwrap();
    let conn = f.app.store.conn().unwrap();
    let status: String = conn
        .query_row("SELECT status FROM jobs WHERE id=?", [&job.id], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(status, "cancelled");
    let n: i64 = conn
        .query_row("SELECT count(*) FROM receipts WHERE key='paid'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(n, 1);
}
#[tokio::test]
async fn membership_migration_is_one_time_and_timezone_change_never_carries_history() {
    let f = Fixture::new();
    let (u, t) = user(&f.app, "A").await;
    f.app
        .store
        .put(
            &u,
            "source",
            &json!({"id":"s","occurredAt":"2020-10-11T23:30:00Z","createdAt":memhot_server::now()}),
            None,
        )
        .unwrap();
    f.app
        .store
        .put(
            &u,
            "thought",
            &json!({"id":"legacy","sourceId":"s","createdAt":memhot_server::now(),"status":"open"}),
            None,
        )
        .unwrap();
    f.app
        .store
        .conn()
        .unwrap()
        .execute_batch("DROP TABLE thought_days")
        .unwrap();
    let migrated = memhot_server::App::open(f.app.data_dir.as_ref().clone()).unwrap();
    let (_, history) = request(&migrated, "GET", "/api/v1/thoughts?scope=history", &t, None).await;
    assert_eq!(history["items"][0]["thinkingDate"], "2020-10-12");
    let mut profile = migrated.store.profile(&u).unwrap();
    profile["timezone"] = json!("America/Los_Angeles");
    migrated.store.save_profile(&u, &profile).unwrap();
    drop(migrated);
    let reopened = memhot_server::App::open(f.app.data_dir.as_ref().clone()).unwrap();
    let (_, history) = request(&reopened, "GET", "/api/v1/thoughts?scope=history", &t, None).await;
    assert_eq!(history["items"][0]["thinkingDate"], "2020-10-12");
    let (_, today) = request(&reopened, "GET", "/api/v1/thoughts?scope=today", &t, None).await;
    assert_eq!(today["total"], 0);
}
#[tokio::test]
async fn unchanged_carried_content_reuses_prior_research_dedup() {
    let f = Fixture::new();
    let (u, t) = user(&f.app, "A").await;
    thought(&f, &u, "old", "2020-10-12", "open");
    let at = chrono::DateTime::parse_from_rfc3339("2020-10-12T15:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    worker::schedule_at(&f.app, at).unwrap();
    f.app
        .store
        .conn()
        .unwrap()
        .execute(
            "UPDATE jobs SET status='completed' WHERE kind='research'",
            [],
        )
        .unwrap();
    request(
        &f.app,
        "POST",
        "/api/v1/thoughts/old/today",
        &t,
        Some(json!({"revision":1})),
    )
    .await;
    let at = chrono::DateTime::parse_from_rfc3339(&format!("{}T15:00:00Z", today()))
        .unwrap()
        .with_timezone(&chrono::Utc);
    worker::schedule_at(&f.app, at).unwrap();
    let conn = f.app.store.conn().unwrap();
    let n: i64 = conn
        .query_row("SELECT count(*) FROM jobs WHERE kind='research'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(n, 1);
    let n: i64 = conn
        .query_row(
            "SELECT count(*) FROM jobs WHERE kind='research' AND status='completed'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n, 1);
}
