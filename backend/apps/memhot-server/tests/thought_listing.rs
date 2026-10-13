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

#[tokio::test]
async fn search_filters_the_whole_collection_and_binds_new_cursors_to_filters() {
    let f = Fixture::new();
    let (a, token) = user(&f.app, "A").await;
    let (b, _) = user(&f.app, "B").await;
    for n in 0..145 {
        let value = json!({"id":format!("t-{n:03}"),"title":if n<5 {"needle"} else {"ordinary"},
            "question":"question","summary":"summary","category":"Learning","status":"open",
            "createdAt":format!("2026-10-12T01:{:02}:{:02}Z",n/60,n%60)});
        f.app.store.put(&a, "thought", &value, None).unwrap();
    }
    f.app.store.put(&b,"thought",&json!({"id":"foreign","title":"needle","category":"private","createdAt":"2026-10-12T01:00:00Z"}),None).unwrap();
    let (status, first) = request(
        &f.app,
        "GET",
        "/api/v1/thoughts?q=needle&limit=2",
        &token,
        None,
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(first["total"], 5);
    assert_eq!(first["counts"]["active"], 5);
    assert_eq!(first["categories"], json!(["Learning"]));
    assert_eq!(
        first["items"][0]["id"], "t-004",
        "search must cover entries beyond the default page"
    );
    let cursor = first["nextCursor"].as_str().unwrap();
    let (_, next) = request(
        &f.app,
        "GET",
        &format!("/api/v1/thoughts?q=needle&limit=2&before={cursor}"),
        &token,
        None,
    )
    .await;
    assert_eq!(next["total"], 5);
    assert_eq!(next["items"][0]["id"], "t-002");
    assert_eq!(
        request(
            &f.app,
            "GET",
            &format!("/api/v1/thoughts?q=ordinary&before={cursor}"),
            &token,
            None
        )
        .await
        .0,
        400
    );
    let final_cursor = next["nextCursor"].as_str().unwrap();
    let (_, last) = request(
        &f.app,
        "GET",
        &format!("/api/v1/thoughts?q=needle&limit=2&before={final_cursor}"),
        &token,
        None,
    )
    .await;
    assert_eq!(last["items"].as_array().unwrap().len(), 1);
    assert!(last["nextCursor"].is_null());
}

#[tokio::test]
async fn literal_search_category_status_and_local_date_combine_without_leaking_facets() {
    let f = Fixture::new();
    let (a, token) = user(&f.app, "A").await;
    let (b, _) = user(&f.app, "B").await;
    let mut profile = f.app.store.profile(&a).unwrap();
    profile["timezone"] = json!("America/Los_Angeles");
    f.app.store.save_profile(&a, &profile).unwrap();
    for (id, status, category, created, field, text) in [
        (
            "active",
            "open",
            "A",
            "2026-10-12T08:00:00Z",
            "title",
            "100%_\\ done",
        ),
        (
            "archived",
            "dismissed",
            "A",
            "2026-10-12T07:00:00Z",
            "quote",
            "100%_\\ done",
        ),
        (
            "resolved",
            "resolved",
            "B",
            "2026-10-12T06:59:59Z",
            "summary",
            "100%_\\ done",
        ),
        (
            "wrong-text",
            "kept",
            "A",
            "2026-10-12T10:00:00Z",
            "question",
            "100 xyz done",
        ),
        (
            "deleted",
            "deleted",
            "hidden",
            "2026-10-13T01:00:00Z",
            "title",
            "100%_\\ done",
        ),
    ] {
        let mut value = json!({"id":id,"status":status,"category":category,"createdAt":created});
        value[field] = json!(text);
        f.app.store.put(&a, "thought", &value, None).unwrap();
    }
    f.app
        .store
        .put(
            &b,
            "thought",
            &json!({"id":"foreign","category":"private","createdAt":"2030-01-01T00:00:00Z"}),
            None,
        )
        .unwrap();
    let (code, data) = request(
        &f.app,
        "GET",
        "/api/v1/thoughts?q=100%25_%5C&status=archived&category=A&date=2026-10-12",
        &token,
        None,
    )
    .await;
    assert_eq!(code, 200);
    assert_eq!(data["total"], 1);
    assert_eq!(data["items"][0]["id"], "archived");
    assert_eq!(data["counts"], json!({"active":1,"archived":1}));
    assert_eq!(data["categories"], json!(["A", "B"]));
    assert_eq!(data["dates"], json!(["2026-10-12", "2026-10-11"]));
    let (_, data) = request(
        &f.app,
        "GET",
        "/api/v1/thoughts?q=100%25&status=active",
        &token,
        None,
    )
    .await;
    assert_eq!(data["total"], 1);
    for path in [
        "/api/v1/thoughts?status=deleted",
        "/api/v1/thoughts?period=year",
        "/api/v1/thoughts?date=2026-02-30",
        "/api/v1/thoughts?date=2026-1-1",
    ] {
        assert_eq!(request(&f.app, "GET", path, &token, None).await.0, 400);
    }
    assert_eq!(
        request(
            &f.app,
            "GET",
            &format!("/api/v1/thoughts?q={}", "a".repeat(201)),
            &token,
            None
        )
        .await
        .0,
        400
    );
}

#[tokio::test]
async fn periods_use_seven_or_thirty_local_calendar_dates_and_intersect_date() {
    use chrono::{Duration, TimeZone, Utc};
    let f = Fixture::new();
    let (a, token) = user(&f.app, "A").await;
    let tz = chrono_tz::Asia::Shanghai;
    let today = Utc::now().with_timezone(&tz).date_naive();
    for days in [0, 6, 7, 29, 30] {
        let created = tz
            .from_local_datetime(
                &(today - Duration::days(days))
                    .and_hms_opt(12, 0, 0)
                    .unwrap(),
            )
            .single()
            .unwrap()
            .to_rfc3339();
        f.app
            .store
            .put(
                &a,
                "thought",
                &json!({"id":format!("d-{days}"),"createdAt":created,"status":"open"}),
                None,
            )
            .unwrap();
    }
    let (_, week) = request(&f.app, "GET", "/api/v1/thoughts?period=week", &token, None).await;
    assert_eq!(week["total"], 2);
    let (_, month) = request(&f.app, "GET", "/api/v1/thoughts?period=month", &token, None).await;
    assert_eq!(month["total"], 4);
    let older = (today - Duration::days(7)).to_string();
    let (_, none) = request(
        &f.app,
        "GET",
        &format!("/api/v1/thoughts?period=week&date={older}"),
        &token,
        None,
    )
    .await;
    assert_eq!(none["total"], 0);
}
