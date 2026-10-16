mod common;
use common::*;
use serde_json::json;

fn put(f: &Fixture, user: &str, key: &str, date: &str) {
    f.app.store.put(user,"thought",&json!({"id":key,"title":key,"question":key,"status":"open","revision":1,"createdAt":format!("{date}T02:00:00Z")}),None).unwrap();
}
fn count(f: &Fixture, user: &str, key: &str) -> i64 {
    f.app
        .store
        .conn()
        .unwrap()
        .query_row(
            "SELECT count(*) FROM thought_days WHERE user_id=? AND thought_id=?",
            rusqlite::params![user, key],
            |r| r.get(0),
        )
        .unwrap()
}
fn today() -> String {
    chrono::Utc::now()
        .with_timezone(&chrono_tz::Asia::Shanghai)
        .date_naive()
        .to_string()
}
#[tokio::test]
async fn hard_delete_cleans_only_owned_memberships_and_reused_id_has_fresh_origin() {
    let f = Fixture::new();
    let (u, t) = user(&f.app, "A").await;
    let (other, _) = user(&f.app, "B").await;
    put(&f, &u, "same", "2020-10-12");
    put(&f, &other, "same", "2020-10-12");
    let (status, carried) = request(
        &f.app,
        "POST",
        "/api/v1/thoughts/same/today",
        &t,
        Some(json!({"revision":1})),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(carried["inToday"], true);
    assert_eq!(count(&f, &u, "same"), 2);
    let stored = f.app.store.get(&u, "thought", "same").unwrap();
    assert!(stored.get("inToday").is_none());
    assert!(stored.get("thinkingDate").is_none());
    assert_eq!(
        request(&f.app, "DELETE", "/api/v1/thoughts/same", &t, None)
            .await
            .0,
        204
    );
    assert_eq!(count(&f, &u, "same"), 0);
    assert_eq!(count(&f, &other, "same"), 1);
    put(&f, &u, "same", "2020-10-13");
    assert_eq!(count(&f, &u, "same"), 1);
    let (_, today) = request(&f.app, "GET", "/api/v1/thoughts?scope=today", &t, None).await;
    assert_eq!(today["total"], 0);
    f.app.store.remove(&u, "thought", "same").unwrap();
    assert_eq!(count(&f, &u, "same"), 0);
    put(&f, &u, "same", "2020-10-14");
    assert_eq!(
        request(&f.app, "DELETE", "/api/v1/account/data", &t, None)
            .await
            .0,
        204
    );
    assert_eq!(count(&f, &u, "same"), 0);
    assert_eq!(count(&f, &other, "same"), 1);
}
#[tokio::test]
async fn existing_membership_schema_installs_delete_trigger_without_reseeding() {
    let f = Fixture::new();
    let (u, _) = user(&f.app, "A").await;
    put(&f, &u, "old", "2020-10-12");
    f.app
        .store
        .conn()
        .unwrap()
        .execute_batch("DROP TRIGGER thought_days_delete")
        .unwrap();
    let reopened = memhot_server::App::open(f.app.data_dir.as_ref().clone()).unwrap();
    let twice = memhot_server::App::open(f.app.data_dir.as_ref().clone()).unwrap();
    drop(twice);
    assert_eq!(count(&f, &u, "old"), 1);
    reopened.store.remove(&u, "thought", "old").unwrap();
    assert_eq!(count(&f, &u, "old"), 0);
}
#[tokio::test]
async fn source_cleanup_and_merge_placeholder_delete_clean_memberships() {
    let f = Fixture::new();
    let (u, _) = user(&f.app, "A").await;
    f.app
        .store
        .put(
            &u,
            "source",
            &json!({"id":"s","createdAt":"2020-10-12T02:00:00Z","status":"ready","text":"输入"}),
            None,
        )
        .unwrap();
    f.app.store.put(&u,"thought",&json!({"id":"sourced","sourceId":"s","sourceIds":["s"],"status":"open","createdAt":"2020-10-12T02:00:00Z"}),None).unwrap();
    assert_eq!(count(&f, &u, "sourced"), 1);
    memhot_server::sources::delete_owned(&f.app, &u, "s").unwrap();
    assert_eq!(count(&f, &u, "sourced"), 0);
    put(&f, &u, "placeholder", "2020-10-12");
    f.app
        .store
        .queue(&u, "formation", json!({}), "merge")
        .unwrap();
    let job = f.app.store.claim().unwrap().unwrap();
    assert!(f
        .app
        .store
        .finish(
            &job,
            &[(
                "merge_seed",
                json!({"id":"placeholder","mergedInto":"existing"})
            )],
            "done"
        )
        .unwrap());
    assert_eq!(count(&f, &u, "placeholder"), 0);
}
#[tokio::test]
async fn home_limits_after_day_and_active_filters_without_capping_summaries() {
    let f = Fixture::new();
    let (u, t) = user(&f.app, "A").await;
    for n in 0..65 {
        put(&f, &u, &format!("live-{n:03}"), &today());
    }
    for n in 0..70 {
        let key = format!("zzz-{n}");
        put(&f, &u, &key, &today());
        let mut v = f.app.store.get(&u, "thought", &key).unwrap();
        v["status"] = json!("dismissed");
        f.app.store.put(&u, "thought", &v, None).unwrap();
    }
    let (status, home) = request(&f.app, "GET", "/api/v1/home?thoughtScope=today", &t, None).await;
    assert_eq!(status, 200);
    assert_eq!(home["thoughts"].as_array().unwrap().len(), 60);
    assert!(home["thoughts"]
        .as_array()
        .unwrap()
        .iter()
        .all(|t| t["inToday"] == true && t["status"] == "open"));
    f.app
        .store
        .queue(&u, "daily_summary", json!({"localDate":today()}), "summary")
        .unwrap();
    let job = f.app.store.claim().unwrap().unwrap();
    let records = memhot_server::daily::run(&f.app, &job).await.unwrap();
    assert_eq!(records[0].1["sections"].as_array().unwrap().len(), 65);
}
