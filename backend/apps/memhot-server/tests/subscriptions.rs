#[allow(dead_code)]
mod common;
use axum::http::StatusCode;
use common::*;
use serde_json::json;
#[tokio::test]
async fn subscription_routes_require_auth_and_do_not_cross_tenants() {
    let f = Fixture::new();
    let app = &f.app;
    for (method, path, body) in [
        ("GET", "/api/v1/subscriptions", None),
        (
            "POST",
            "/api/v1/subscriptions",
            Some(json!({"url":"https://example.com/feed"})),
        ),
        (
            "POST",
            "/api/v1/subscriptions/import",
            Some(json!({"feeds":[{"url":"https://example.com/feed"}]})),
        ),
        (
            "PUT",
            "/api/v1/subscriptions/nonexistent",
            Some(json!({"enabled":false})),
        ),
        ("DELETE", "/api/v1/subscriptions/nonexistent", None),
        ("POST", "/api/v1/subscriptions/nonexistent/refresh", None),
        ("GET", "/api/v1/subscriptions/nonexistent/items", None),
    ] {
        assert_eq!(
            request(app, method, path, "", body).await.0,
            StatusCode::UNAUTHORIZED,
            "{path}"
        );
    }
    let (_, a) = user(app, "A").await;
    let (_, b) = user(app, "B").await;
    let (status, created) = request(
        app,
        "POST",
        "/api/v1/subscriptions",
        &a,
        Some(json!({"url":"https://example.com/feed?token=private","category":""})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(created["category"], "其他");
    assert!(!created.to_string().contains("private"));
    let id = created["id"].as_str().unwrap();
    for (method, path, body) in [
        (
            "PUT",
            format!("/api/v1/subscriptions/{id}"),
            Some(json!({"enabled":false})),
        ),
        ("DELETE", format!("/api/v1/subscriptions/{id}"), None),
        ("POST", format!("/api/v1/subscriptions/{id}/refresh"), None),
        ("GET", format!("/api/v1/subscriptions/{id}/items"), None),
    ] {
        assert_eq!(
            request(app, method, &path, &b, body).await.0,
            StatusCode::NOT_FOUND,
            "{path}"
        );
    }
    assert_eq!(
        request(app, "GET", "/api/v1/subscriptions", &b, None)
            .await
            .1["subscriptions"],
        json!([])
    );
    assert_eq!(
        request(
            app,
            "POST",
            &format!("/api/v1/subscriptions/{id}/refresh"),
            &a,
            None
        )
        .await
        .1["queued"],
        true
    );
    assert_eq!(
        request(
            app,
            "DELETE",
            &format!("/api/v1/subscriptions/{id}"),
            &a,
            None
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
}
#[tokio::test]
async fn legacy_profile_contract_never_deletes_managed_sources_and_clear_removes_cache() {
    let f = Fixture::new();
    let app = &f.app;
    let (uid, token) = user(app, "A").await;
    let (status, _) = request(
        app,
        "PUT",
        "/api/v1/profile",
        &token,
        Some(json!({"feeds":["https://example.com/legacy?secret=private"]})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, managed) = request(
        app,
        "POST",
        "/api/v1/subscriptions",
        &token,
        Some(json!({"url":"https://example.com/managed"})),
    )
    .await;
    assert_eq!(
        request(
            app,
            "PUT",
            "/api/v1/profile",
            &token,
            Some(json!({"feeds":[]}))
        )
        .await
        .0,
        StatusCode::OK
    );
    let listing = request(app, "GET", "/api/v1/subscriptions", &token, None)
        .await
        .1;
    assert_eq!(listing["subscriptions"].as_array().unwrap().len(), 1);
    assert_eq!(listing["subscriptions"][0]["id"], managed["id"]);
    assert_eq!(app.store.profile(&uid).unwrap()["feeds"], json!([]));
    assert_eq!(
        request(app, "DELETE", "/api/v1/account/data", &token, None)
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        request(app, "GET", "/api/v1/subscriptions", &token, None)
            .await
            .1["subscriptions"],
        json!([])
    );
}
