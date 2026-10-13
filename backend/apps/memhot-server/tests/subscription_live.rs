//! Opt-in public-network smoke test. Uses temporary local tenants, no production credentials or models.
//! Run: cargo test -p memhot-server --test subscription_live -- --ignored --nocapture
#[allow(dead_code)]
mod common;
use axum::http::StatusCode;
use common::{request, user, Fixture};
use memhot_server::{subscription_items, subscription_worker, App};
use serde_json::{json, Value};
use std::time::Duration;

struct Worker(tokio::task::JoinHandle<()>);
impl Drop for Worker {
    fn drop(&mut self) {
        self.0.abort();
    }
}
fn count(app: &App, table: &str) -> i64 {
    app.store
        .conn()
        .unwrap()
        .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}
async fn success(app: &App, token: &str, after: Option<&str>) -> Value {
    tokio::time::timeout(Duration::from_secs(100), async {
        loop {
            let (status, listing) = request(app, "GET", "/api/v1/subscriptions", token, None).await;
            assert_eq!(status, StatusCode::OK);
            let source = listing["subscriptions"][0].clone();
            if source["lastSuccessAt"]
                .as_str()
                .is_some_and(|at| Some(at) != after)
            {
                assert_eq!(source["status"], "healthy");
                return source;
            }
            assert_ne!(
                source["status"], "error",
                "live public feed failed: {}",
                source["error"]
            );
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    })
    .await
    .expect("collector did not complete within its bounded fetch/cooldown window")
}

#[tokio::test]
#[ignore = "requires real public RSS network access; no model calls"]
async fn federal_reserve_authenticated_collection_and_shared_cache() {
    let fixture = Fixture::new();
    let app = &fixture.app;
    let (a, token_a) = user(app, "Live smoke A").await;
    let (b, token_b) = user(app, "Live smoke B").await;
    assert_eq!(
        request(app, "GET", "/api/v1/subscriptions/catalog", "", None)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let (status, catalog) =
        request(app, "GET", "/api/v1/subscriptions/catalog", &token_a, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(catalog["sources"]
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s["id"] == "federal-reserve"));
    let (status, preview) = request(
        app,
        "POST",
        "/api/v1/subscriptions/preview",
        &token_a,
        Some(json!({"catalogId":"federal-reserve"})),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "public preview failed: {}",
        preview["error"]["code"]
    );
    assert!(preview["candidates"]
        .as_array()
        .is_some_and(|c| !c.is_empty()));
    assert_eq!(
        count(app, "subscriptions"),
        0,
        "preview must never subscribe"
    );
    let (status, created) = request(
        app,
        "POST",
        "/api/v1/subscriptions",
        &token_a,
        Some(json!({"catalogId":"federal-reserve"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(created["shared"], true);
    assert!(created.get("url").is_none());
    let id_a = created["id"].as_str().unwrap();
    let worker = Worker(tokio::spawn(subscription_worker::run(app.clone())));
    let first = success(app, &token_a, None).await;
    let first_success = first["lastSuccessAt"].as_str().unwrap();
    let (status, detail) = request(
        app,
        "GET",
        &format!("/api/v1/subscriptions/{id_a}/items"),
        &token_a,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let rows = detail["items"].as_array().unwrap();
    assert!(
        !rows.is_empty(),
        "Federal Reserve feed should expose public release summaries"
    );
    let dates: Vec<_> = rows
        .iter()
        .filter_map(|item| item["publishedAt"].as_str())
        .filter_map(|date| {
            chrono::DateTime::parse_from_rfc2822(date)
                .or_else(|_| chrono::DateTime::parse_from_rfc3339(date))
                .ok()
        })
        .collect();
    assert!(
        !dates.is_empty(),
        "original release dates must remain available"
    );
    let oldest = dates.iter().min().unwrap();
    let newest = dates.iter().max().unwrap();
    let discovered =
        chrono::DateTime::parse_from_rfc3339(rows[0]["discoveredAt"].as_str().unwrap()).unwrap();
    assert!(
        *oldest < discovered - chrono::Duration::days(1),
        "initial backfill must retain historical publication dates"
    );
    assert!(rows
        .iter()
        .all(|item| item["publishedAt"] != item["discoveredAt"]));
    let initial_count = rows.len();
    println!("Federal Reserve live smoke: collected={initial_count}, dated={}, oldestPublished={}, newestPublished={}, discoveredAt={}; no content bodies or credentials logged", dates.len(),oldest.to_rfc3339(),newest.to_rfc3339(),discovered.to_rfc3339());
    assert!(subscription_items::materials(app, &b).unwrap().is_empty());
    for (method, suffix, body) in [
        ("GET", "/items", None),
        ("POST", "/refresh", None),
        ("PUT", "", Some(json!({"enabled":false}))),
        ("DELETE", "", None),
    ] {
        assert_eq!(
            request(
                app,
                method,
                &format!("/api/v1/subscriptions/{id_a}{suffix}"),
                &token_b,
                body
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
    }
    let (status, shared) = request(
        app,
        "POST",
        "/api/v1/subscriptions",
        &token_b,
        Some(json!({"catalogId":"federal-reserve"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let id_b = shared["id"].as_str().unwrap();
    assert_ne!(id_a, id_b);
    assert_eq!(
        count(app, "subscription_sources"),
        1,
        "curated subscribers must share one collection source"
    );
    assert_eq!(shared["lastSuccessAt"], first["lastSuccessAt"]);
    assert_eq!(shared["itemCount"], first["itemCount"]);
    assert_eq!(
        subscription_items::materials(app, &b).unwrap().len(),
        initial_count
    );
    assert_eq!(
        subscription_items::public_materials(app).unwrap().len(),
        initial_count
    );
    let (status, refresh) = request(
        app,
        "POST",
        &format!("/api/v1/subscriptions/{id_a}/refresh"),
        &token_a,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(refresh["queued"], true);
    let (_, duplicate) = request(
        app,
        "POST",
        &format!("/api/v1/subscriptions/{id_a}/refresh"),
        &token_a,
        None,
    )
    .await;
    assert_eq!(duplicate["nextAttemptAt"], refresh["nextAttemptAt"]);
    let second = success(app, &token_a, Some(first_success)).await;
    assert_ne!(second["lastSuccessAt"], first["lastSuccessAt"]);
    let retained: i64 = app
        .store
        .conn()
        .unwrap()
        .query_row(
            "SELECT count(*) FROM subscription_items WHERE discovered=?",
            [rows[0]["discoveredAt"].as_str().unwrap()],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        retained > 0,
        "refresh must preserve first discovery timestamps"
    );
    println!("Refresh completed: retainedFirstDiscovery={retained}, newlyCollected={}; shared source count=1",second["lastNewCount"]);
    assert_eq!(
        request(
            app,
            "PUT",
            &format!("/api/v1/subscriptions/{id_a}"),
            &token_a,
            Some(json!({"enabled":false}))
        )
        .await
        .0,
        StatusCode::OK
    );
    assert!(subscription_items::materials(app, &a).unwrap().is_empty());
    assert!(!subscription_items::materials(app, &b).unwrap().is_empty());
    assert_eq!(
        request(
            app,
            "DELETE",
            &format!("/api/v1/subscriptions/{id_a}"),
            &token_a,
            None
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(count(app, "subscription_sources"), 1);
    assert_eq!(
        request(
            app,
            "DELETE",
            &format!("/api/v1/subscriptions/{id_b}"),
            &token_b,
            None
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(count(app, "subscription_sources"), 0);
    assert_eq!(count(app, "subscription_items"), 0);
    assert_eq!(count(app, "jobs"), 0);
    assert_eq!(count(app, "receipts"), 0);
    drop(worker);
    println!("Pause/delete and cross-tenant isolation verified; jobs=0, model receipts=0; temporary database removed on exit.");
}
