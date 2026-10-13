mod common;
use axum::http::StatusCode;
use common::*;
use serde_json::json;
#[tokio::test]
async fn pagination_covers_all_topics_with_timestamp_ties_without_crossing_users() {
    let f = Fixture::new();
    let (a, token) = user(&f.app, "A").await;
    let (b, other) = user(&f.app, "B").await;
    for n in 0..123 {
        f.app.store.put(&a,"thought",&json!({"id":format!("topic-{n:03}"),"status":"open","createdAt":"2026-10-12T01:00:00Z"}),None).unwrap();
    }
    f.app
        .store
        .put(
            &a,
            "thought",
            &json!({"id":"deleted","status":"deleted","createdAt":"2026-10-13T01:00:00Z"}),
            None,
        )
        .unwrap();
    f.app
        .store
        .put(
            &b,
            "thought",
            &json!({"id":"private-b","createdAt":"2026-10-12T01:00:00Z"}),
            None,
        )
        .unwrap();
    let mut seen = std::collections::HashSet::new();
    let mut path = "/api/v1/thoughts?limit=50".to_owned();
    for page in 0..3 {
        let (code, data) = request(&f.app, "GET", &path, &token, None).await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(
            data["items"].as_array().unwrap().len(),
            if page < 2 { 50 } else { 23 }
        );
        for item in data["items"].as_array().unwrap() {
            assert!(seen.insert(item["id"].as_str().unwrap().to_owned()));
        }
        if let Some(cursor) = data["nextCursor"].as_str() {
            path = format!("/api/v1/thoughts?limit=50&before={cursor}");
            let (_, foreign) = request(&f.app, "GET", &path, &other, None).await;
            assert!(foreign["items"]
                .as_array()
                .unwrap()
                .iter()
                .all(|v| v["id"] == "private-b"));
        } else {
            assert_eq!(page, 2);
        }
    }
    assert_eq!(seen.len(), 123);
    assert!(!seen.contains("deleted"));
    assert!(!seen.contains("private-b"));
    assert_eq!(
        request(
            &f.app,
            "GET",
            "/api/v1/thoughts?before=not_json",
            &token,
            None
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
}
