mod common;
use common::*;
use serde_json::json;

#[tokio::test]
async fn competing_edits_have_one_winner_and_retried_deletions_have_one_memory() {
    let f = Fixture::new();
    let (uid, token) = user(&f.app, "A").await;
    let (_, thought) = request(
        &f.app,
        "POST",
        "/api/v1/thoughts",
        &token,
        Some(json!({"question":"原始问题"})),
    )
    .await;
    let key = thought["id"].as_str().unwrap();
    let path = format!("/api/v1/thoughts/{key}");
    let (first, second) = tokio::join!(
        request(
            &f.app,
            "PUT",
            &path,
            &token,
            Some(json!({"question":"修改甲","revision":1}))
        ),
        request(
            &f.app,
            "PUT",
            &path,
            &token,
            Some(json!({"question":"修改乙","revision":1}))
        )
    );
    let mut statuses = [first.0.as_u16(), second.0.as_u16()];
    statuses.sort();
    assert_eq!(statuses, [200, 409]);
    let current = f.app.store.get(&uid, "thought", key).unwrap();
    assert_eq!(current["revision"], 2);
    let path = format!("{path}?preference=less&revision=2");
    let (first, second) = tokio::join!(
        request(&f.app, "DELETE", &path, &token, None),
        request(&f.app, "DELETE", &path, &token, None)
    );
    assert_eq!(first.0, 204);
    assert_eq!(second.0, 204);
    let cognitions = f.app.store.list_all(&uid, "cognition").unwrap();
    assert_eq!(cognitions.len(), 1);
    assert_eq!(cognitions[0]["revision"], 1);
    assert_eq!(cognitions[0]["scope"]["question"], current["question"]);
}

#[tokio::test]
async fn legacy_deleted_rows_cannot_be_restored_by_edit_or_feedback() {
    let f = Fixture::new();
    let (uid, token) = user(&f.app, "A").await;
    f.app
        .store
        .put(
            &uid,
            "thought",
            &json!({"id":"gone","status":"deleted","createdAt":memhot_server::now()}),
            None,
        )
        .unwrap();
    assert_eq!(
        request(
            &f.app,
            "PUT",
            "/api/v1/thoughts/gone",
            &token,
            Some(json!({"status":"open"}))
        )
        .await
        .0,
        404
    );
    assert_eq!(
        request(
            &f.app,
            "POST",
            "/api/v1/thoughts/gone/feedback",
            &token,
            Some(json!({"action":"keep"}))
        )
        .await
        .0,
        404
    );
    assert_eq!(
        f.app.store.get(&uid, "thought", "gone").unwrap()["status"],
        "deleted"
    );
}

#[tokio::test]
async fn invalid_create_does_not_leave_an_interest_or_partial_thought() {
    let f = Fixture::new();
    let (uid, token) = user(&f.app, "A").await;
    assert_eq!(
        request(
            &f.app,
            "POST",
            "/api/v1/thoughts",
            &token,
            Some(json!({"question":"问题","sourceIds":["nonexistent"]}))
        )
        .await
        .0,
        404
    );
    assert!(f.app.store.list_all(&uid, "thought").unwrap().is_empty());
    assert!(f.app.store.list_all(&uid, "cognition").unwrap().is_empty());
}
