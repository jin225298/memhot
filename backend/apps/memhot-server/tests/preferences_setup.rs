mod common;
use axum::{extract::State, Json};
use common::*;
use memhot_server::{preferences_setup, App};
use serde_json::{json, Value};
const PATH: &str = "/api/v1/preferences/setup";

#[tokio::test]
async fn catalog_is_tenant_scoped_secret_free_and_uses_exact_public_aliases() {
    let f = Fixture::new();
    let (_, token) = user(&f.app, "A").await;
    assert_eq!(request(&f.app, "GET", PATH, "", None).await.0, 401);
    let (status, body) = request(&f.app, "GET", PATH, &token, None).await;
    assert_eq!(status, 200);
    assert_eq!(body["completed"], false);
    assert_eq!(body["interests"].as_array().unwrap().len(), 12);
    assert_eq!(body["reading"].as_array().unwrap().len(), 5);
    assert_eq!(body["thinking"].as_array().unwrap().len(), 4);
    let values: Vec<String> = body["interests"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["value"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(memhot_server::shared_public::match_topics(&values).len(), 4);
    assert_eq!(body.as_object().unwrap().len(), 4);
}
#[tokio::test]
async fn setup_merges_existing_data_preserves_credentials_and_replay_cannot_override_edits() {
    let f = Fixture::new();
    let (uid, token) = user(&f.app, "A").await;
    let mut profile = f.app.store.profile(&uid).unwrap();
    profile["interests"] = json!(["自定义领域", " 自定义领域 "]);
    profile["timezone"] = json!("Europe/London");
    profile["models"] = json!({"text":{"apiKey":"retained-secret-fixture"}});
    profile["search"] = json!({"apiKey":"retained-search-fixture"});
    f.app.store.save_profile(&uid, &profile).unwrap();
    f.app.store.put(&uid,"cognition",&json!({"id":"existing","type":"goal","statement":"已有手工目标","origin":"manual","status":"active"}),None).unwrap();
    assert_eq!(
        request(&f.app, "GET", PATH, &token, None).await.1["completed"],
        true
    );
    let input = json!({"interestIds":["ai","ai"],"customInterests":[" 自定义领域 ","新领域","新领域"],"readingIds":["summary","data"],"thinkingIds":["counter"]});
    let (status, result) = request(&f.app, "POST", PATH, &token, Some(input.clone())).await;
    assert_eq!(status, 200);
    assert_eq!(result["completed"], true);
    assert!(!result.to_string().contains("retained-"));
    let saved = f.app.store.profile(&uid).unwrap();
    assert_eq!(
        saved["interests"],
        json!(["自定义领域", "人工智能", "新领域"])
    );
    for key in [
        "timezone",
        "models",
        "search",
        "name",
        "researchHour",
        "digestHour",
    ] {
        assert_eq!(saved[key], profile[key]);
    }
    let cognition = f.app.store.list_all(&uid, "cognition").unwrap();
    assert_eq!(cognition.len(), 4);
    for c in cognition.iter().filter(|c| c["id"] != "existing") {
        assert_eq!(c["origin"], "manual");
        assert_eq!(c["status"], "active");
        assert_eq!(c["type"], "preference");
        assert_eq!(c["confidence"], 1.0);
    }
    assert_eq!(
        request(
            &f.app,
            "PUT",
            "/api/v1/profile",
            &token,
            Some(json!({"interests":["后来编辑的兴趣"]}))
        )
        .await
        .0,
        200
    );
    assert_eq!(
        request(
            &f.app,
            "PUT",
            "/api/v1/cognition/setup-preference-reading-summary",
            &token,
            Some(json!({"statement":"后来编辑的偏好","revision":1}))
        )
        .await
        .0,
        200
    );
    assert_eq!(
        request(&f.app, "POST", PATH, &token, Some(input)).await.0,
        200
    );
    assert_eq!(
        f.app.store.profile(&uid).unwrap()["interests"],
        json!(["后来编辑的兴趣"])
    );
    assert_eq!(
        f.app
            .store
            .get(&uid, "cognition", "setup-preference-reading-summary")
            .unwrap()["statement"],
        "后来编辑的偏好"
    );
    assert_eq!(f.app.store.list_all(&uid, "cognition").unwrap().len(), 4);
}
#[tokio::test]
async fn validation_is_atomic_and_skip_only_sets_marker() {
    let f = Fixture::new();
    let (uid, token) = user(&f.app, "A").await;
    let before = f.app.store.profile(&uid).unwrap();
    for invalid in [
        json!({"interestIds":["unknown"]}),
        json!({"readingIds":["unknown"]}),
        json!({"thinkingIds":["unknown"]}),
        json!({"skip":"yes"}),
        json!({"customInterests":["x".repeat(81)]}),
        json!({"customInterests":[""]}),
        json!({"interestIds":null}),
        json!({"unexpected":true}),
    ] {
        assert_eq!(
            request(&f.app, "POST", PATH, &token, Some(invalid)).await.0,
            400
        );
        assert_eq!(f.app.store.profile(&uid).unwrap(), before);
        assert!(f.app.store.list_all(&uid, "cognition").unwrap().is_empty());
    }
    let mut full = before.clone();
    full["interests"] = json!((0..20).map(|n| format!("领域{n}")).collect::<Vec<_>>());
    f.app.store.save_profile(&uid, &full).unwrap();
    assert_eq!(
        request(
            &f.app,
            "POST",
            PATH,
            &token,
            Some(json!({"interestIds":["ai"],"readingIds":["summary"]}))
        )
        .await
        .0,
        400
    );
    assert_eq!(f.app.store.profile(&uid).unwrap(), full);
    assert!(f.app.store.list_all(&uid, "cognition").unwrap().is_empty());
    assert_eq!(
        request(
            &f.app,
            "POST",
            PATH,
            &token,
            Some(json!({"skip":true,"interestIds":["ai"],"readingIds":["summary"]}))
        )
        .await
        .0,
        200
    );
    full["preferencesOnboarded"] = json!(true);
    assert_eq!(f.app.store.profile(&uid).unwrap(), full);
    assert!(f.app.store.list_all(&uid, "cognition").unwrap().is_empty());
    assert!(f.app.store.jobs(&uid).unwrap().is_empty());
}
#[tokio::test]
async fn existing_valid_cognition_skips_prompt_but_dismissed_does_not_and_users_are_isolated() {
    let f = Fixture::new();
    let (a, ta) = user(&f.app, "A").await;
    let (b, tb) = user(&f.app, "B").await;
    let pending =
        json!({"id":"c","type":"preference","statement":"待确认的偏好","status":"pending"});
    f.app.store.put(&a, "cognition", &pending, None).unwrap();
    assert_eq!(
        request(&f.app, "GET", PATH, &ta, None).await.1["completed"],
        true
    );
    assert_eq!(
        request(&f.app, "GET", PATH, &tb, None).await.1["completed"],
        false
    );
    let mut dismissed = pending;
    dismissed["status"] = json!("dismissed");
    f.app.store.put(&a, "cognition", &dismissed, None).unwrap();
    assert_eq!(
        request(&f.app, "GET", PATH, &ta, None).await.1["completed"],
        false
    );
    assert_eq!(
        request(
            &f.app,
            "POST",
            PATH,
            &ta,
            Some(json!({"readingIds":["summary"]}))
        )
        .await
        .0,
        200
    );
    assert!(f.app.store.list_all(&b, "cognition").unwrap().is_empty());
    assert_eq!(
        f.app.store.profile(&b).unwrap()["preferencesOnboarded"],
        Value::Null
    );
}
#[tokio::test]
async fn independent_database_connections_allow_only_one_setup_winner() {
    let f = Fixture::new();
    let (uid, _) = user(&f.app, "A").await;
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
    let mut threads = Vec::new();
    for (interest, reading) in [("ai", "summary"), ("physics", "data")] {
        let app = App::open((*f.app.data_dir).clone()).unwrap();
        let uid = uid.clone();
        let barrier = barrier.clone();
        threads.push(std::thread::spawn(move || {
            barrier.wait();
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async {
                    preferences_setup::submit(
                        State(app),
                        memhot_server::auth::User(uid),
                        Json(json!({"interestIds":[interest],"readingIds":[reading]})),
                    )
                    .await
                    .unwrap()
                    .0
                })
        }));
    }
    barrier.wait();
    for thread in threads {
        assert_eq!(thread.join().unwrap()["completed"], true);
    }
    assert_eq!(
        f.app.store.profile(&uid).unwrap()["interests"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(f.app.store.list_all(&uid, "cognition").unwrap().len(), 1);
}
#[tokio::test]
async fn failed_second_cognition_write_rolls_back_interests_and_first_write() {
    let f = Fixture::new();
    let (uid, token) = user(&f.app, "A").await;
    let before = f.app.store.profile(&uid).unwrap();
    f.app.store.conn().unwrap().execute_batch("CREATE TRIGGER reject_setup_fixture BEFORE INSERT ON records WHEN NEW.kind='cognition' AND NEW.id='setup-preference-reading-data' BEGIN SELECT RAISE(ABORT,'fixture'); END;").unwrap();
    assert_eq!(
        request(
            &f.app,
            "POST",
            PATH,
            &token,
            Some(json!({"interestIds":["ai"],"readingIds":["summary","data"]}))
        )
        .await
        .0,
        500
    );
    assert_eq!(f.app.store.profile(&uid).unwrap(), before);
    assert!(f.app.store.list_all(&uid, "cognition").unwrap().is_empty());
    assert_eq!(
        request(&f.app, "GET", PATH, &token, None).await.1["completed"],
        false
    );
}
