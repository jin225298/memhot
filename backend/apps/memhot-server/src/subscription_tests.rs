use crate::auth::User;
use crate::{
    subscription_items as items, subscription_store as store, subscription_worker as worker,
    subscriptions as api, App,
};
use axum::{
    extract::{Path, State},
    Json,
};
use rusqlite::params;
use serde_json::json;
struct Fixture {
    app: App,
    a: String,
    b: String,
}
impl Fixture {
    fn new() -> Self {
        let app =
            App::open(std::env::temp_dir().join(format!("memhot-subscriptions-{}", crate::id())))
                .unwrap();
        let (a, _) = crate::auth::create_pair(&app, "A", None).unwrap();
        let (b, _) = crate::auth::create_pair(&app, "B", None).unwrap();
        Self { app, a, b }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(self.app.data_dir.as_ref());
    }
}
fn add(f: &Fixture, user: &str, url: &str) -> String {
    store::add(&f.app, user, url, None, None, None).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned()
}
fn article() -> crate::discovery::Material {
    crate::feed::parse_from("<rss><channel><item><title>Historical facts</title><link>https://example.com/old</link><description>Original published evidence</description><pubDate>Wed, 01 Jan 2020 12:00:00 +0000</pubDate></item></channel></rss>",Some("https://example.com/private?token=secret")).remove(0)
}
fn count(f: &Fixture, table: &str) -> i64 {
    f.app
        .store
        .conn()
        .unwrap()
        .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

#[tokio::test]
async fn tenant_isolation_private_urls_and_crud() {
    let f = Fixture::new();
    let secret = "https://example.com/private-secret-path?token=secret";
    let a = add(&f, &f.a, secret);
    let b = add(&f, &f.b, secret);
    assert_eq!(count(&f, "subscription_sources"), 2);
    assert_eq!(add(&f, &f.a, secret), a);
    assert!(store::get(&f.app, &f.b, &a).is_err());
    assert!(api::enqueue(&f.app, &f.b, &a, 10).is_err());
    assert!(api::update(
        State(f.app.clone()),
        User(f.b.clone()),
        Path(a.clone()),
        Json(json!({"title":"stolen"}))
    )
    .await
    .is_err());
    assert!(
        api::delete(State(f.app.clone()), User(f.b.clone()), Path(a.clone()))
            .await
            .is_err()
    );
    let list = store::list(&f.app, &f.a).unwrap();
    assert_eq!(list[0]["host"], "example.com");
    assert_eq!(list[0]["shared"], false);
    assert!(!serde_json::to_string(&list).unwrap().contains("secret"));
    let at = chrono::Utc::now().timestamp();
    while let Some(c) = worker::claim(&f.app, at).unwrap() {
        worker::finish(&f.app, &c, Ok(vec![article()]), at + 1).unwrap();
    }
    assert_eq!(items::materials(&f.app, &f.a).unwrap().len(), 1);
    assert_eq!(items::materials(&f.app, &f.b).unwrap().len(), 1);
    assert!(items::public_materials(&f.app).unwrap().is_empty());
    assert_eq!(
        items::materials(&f.app, &f.a).unwrap()[0]
            .discovered_via
            .as_deref(),
        Some("example.com")
    );
    api::delete(State(f.app.clone()), User(f.a.clone()), Path(a))
        .await
        .unwrap();
    assert_eq!(count(&f, "subscription_sources"), 1);
    assert_eq!(count(&f, "subscription_items"), 1);
    assert_eq!(store::get(&f.app, &f.b, &b).unwrap()["itemCount"], 1);
}
#[test]
fn catalog_shares_one_fetch_but_custom_same_url_is_private() {
    let f = Fixture::new();
    let c = &crate::subscription_catalog::entries()[0];
    let a = store::add(&f.app, &f.a, c.url, None, None, Some(c.id)).unwrap();
    let b = store::add(&f.app, &f.b, c.url, None, None, Some(c.id)).unwrap();
    assert_ne!(a["id"], b["id"]);
    assert_eq!(count(&f, "subscription_sources"), 1);
    let at = chrono::Utc::now().timestamp();
    let claim = worker::claim(&f.app, at).unwrap().unwrap();
    assert!(worker::claim(&f.app, at).unwrap().is_none());
    worker::finish(&f.app, &claim, Ok(vec![article()]), at + 1).unwrap();
    assert_eq!(items::materials(&f.app, &f.a).unwrap().len(), 1);
    assert_eq!(items::materials(&f.app, &f.b).unwrap().len(), 1);
    assert_eq!(items::public_materials(&f.app).unwrap().len(), 1);
    add(&f, &f.a, c.url);
    assert_eq!(count(&f, "subscription_sources"), 2);
    assert!(store::add(
        &f.app,
        &f.a,
        "https://example.com/evil",
        None,
        None,
        Some(c.id)
    )
    .is_err());
}
#[test]
fn simultaneous_enqueue_and_claim_are_coalesced_and_cancelled_leases_expire() {
    let f = Fixture::new();
    let id = add(&f, &f.a, "https://example.com/feed");
    let at = chrono::Utc::now().timestamp();
    std::thread::scope(|s| {
        for _ in 0..8 {
            s.spawn(|| {
                assert_eq!(api::enqueue(&f.app, &f.a, &id, at).unwrap()["queued"], true);
            });
        }
    });
    let first = worker::claim(&f.app, at).unwrap().unwrap();
    assert!(worker::claim(&f.app, at + 1).unwrap().is_none());
    assert_eq!(
        api::enqueue(&f.app, &f.a, &id, at + 1).unwrap()["coalesced"],
        true
    );
    // A cancelled worker owns only an expiring token; replacement cannot be overwritten.
    let second = worker::claim(&f.app, at + worker::LEASE_SECONDS)
        .unwrap()
        .unwrap();
    assert_ne!(first.token, second.token);
    assert!(!worker::finish(
        &f.app,
        &first,
        Ok(vec![article()]),
        at + worker::LEASE_SECONDS + 1
    )
    .unwrap());
    assert!(worker::finish(
        &f.app,
        &second,
        Ok(vec![article()]),
        at + worker::LEASE_SECONDS + 1
    )
    .unwrap());
    let refresh = api::enqueue(&f.app, &f.a, &id, at + worker::LEASE_SECONDS + 2).unwrap();
    assert_eq!(store::get(&f.app, &f.a, &id).unwrap()["status"], "pending");
    assert_eq!(
        refresh["cooldownUntil"],
        store::timestamp(Some(at + worker::LEASE_SECONDS + 60)).unwrap()
    );
    assert!(worker::claim(&f.app, at + worker::LEASE_SECONDS + 59)
        .unwrap()
        .is_none());
    assert!(worker::claim(&f.app, at + worker::LEASE_SECONDS + 60)
        .unwrap()
        .is_some());
    assert_eq!(count(&f, "jobs"), 0);
}
#[test]
fn failure_preserves_success_and_items_without_leaking_error_url() {
    let f = Fixture::new();
    let id = add(&f, &f.a, "https://example.com/feed");
    let at = chrono::Utc::now().timestamp();
    let claim = worker::claim(&f.app, at).unwrap().unwrap();
    worker::finish(&f.app, &claim, Ok(vec![article()]), at + 1).unwrap();
    api::enqueue(&f.app, &f.a, &id, at + 61).unwrap();
    let next = worker::claim(&f.app, at + 61).unwrap().unwrap();
    worker::finish(
        &f.app,
        &next,
        Err(crate::error::Error::upstream(
            "network",
            "https://example.com/?token=secret",
        )),
        at + 62,
    )
    .unwrap();
    let row = store::get(&f.app, &f.a, &id).unwrap();
    assert_eq!(
        row["lastSuccessAt"],
        store::timestamp(Some(at + 1)).unwrap()
    );
    assert_eq!(row["itemCount"], 1);
    assert_eq!(row["status"], "error");
    assert!(!row.to_string().contains("secret"));
    assert!(worker::claim(&f.app, at + 63).unwrap().is_none());
}
#[tokio::test]
async fn disable_delete_and_reenable_reject_old_inflight_results() {
    let f = Fixture::new();
    let id = add(&f, &f.a, "https://example.com/feed");
    let at = chrono::Utc::now().timestamp();
    let old = worker::claim(&f.app, at).unwrap().unwrap();
    let _ = api::update(
        State(f.app.clone()),
        User(f.a.clone()),
        Path(id.clone()),
        Json(json!({"enabled":false})),
    )
    .await
    .unwrap();
    assert!(!worker::finish(&f.app, &old, Ok(vec![article()]), at + 1).unwrap());
    assert!(worker::claim(&f.app, at + 2).unwrap().is_none());
    assert!(items::materials(&f.app, &f.a).unwrap().is_empty());
    let _ = api::update(
        State(f.app.clone()),
        User(f.a.clone()),
        Path(id.clone()),
        Json(json!({"enabled":true})),
    )
    .await
    .unwrap();
    assert!(worker::claim(&f.app, at + 3).unwrap().is_none());
    let new = worker::claim(&f.app, at + 61).unwrap().unwrap();
    assert_ne!(old.token, new.token);
    api::delete(State(f.app.clone()), User(f.a.clone()), Path(id))
        .await
        .unwrap();
    assert!(!worker::finish(&f.app, &new, Ok(vec![article()]), at + 62).unwrap());
    assert_eq!(count(&f, "subscription_items"), 0);
}
#[test]
fn historical_backfill_preserves_publication_unknown_dates_and_ttl() {
    let f = Fixture::new();
    add(&f, &f.a, "https://example.com/feed");
    let at = chrono::Utc::now().timestamp();
    let mut unknown = article();
    unknown.url = "https://example.com/unknown".into();
    unknown.published = None;
    let claim = worker::claim(&f.app, at).unwrap().unwrap();
    worker::finish(&f.app, &claim, Ok(vec![article(), unknown]), at + 1).unwrap();
    let materials = items::materials(&f.app, &f.a).unwrap();
    assert_eq!(materials.len(), 2);
    assert!(materials
        .iter()
        .any(|m| m.published.as_deref() == Some("2020-01-01T12:00:00+00:00")));
    assert!(materials.iter().any(|m| m.published.is_none()));
    assert!(materials
        .iter()
        .all(|m| m.discovered.as_deref() == store::timestamp(Some(at + 1)).as_deref()));
    let mut fresh = materials;
    crate::freshness::prepare(&mut fresh, &Default::default(), chrono::Utc::now());
    crate::freshness::select(&mut fresh, &Default::default(), chrono::Utc::now());
    assert!(fresh.is_empty());
    items::retention(&f.app, at + 1 + items::RETENTION_SECONDS).unwrap();
    assert_eq!(count(&f, "subscription_items"), 0);
}
#[test]
fn legacy_migrates_once_and_profile_replacement_preserves_managed_rows() {
    let f = Fixture::new();
    let mut p = f.app.store.profile(&f.a).unwrap();
    p["feeds"] = json!(["https://example.com/legacy"]);
    f.app.store.save_profile(&f.a, &p).unwrap();
    store::migrate_legacy(&f.app, &f.a).unwrap();
    assert_eq!(store::list(&f.app, &f.a).unwrap().len(), 1);
    let managed = add(&f, &f.a, "https://example.com/managed");
    p = f.app.store.profile(&f.a).unwrap();
    let original = p.to_string();
    p["feeds"] = json!([]);
    p["name"] = json!("Changed");
    store::update_profile(
        &f.app,
        &f.a,
        &original,
        &p,
        Some(&json!(["https://example.com/replacement"])),
    )
    .unwrap();
    assert_eq!(store::list(&f.app, &f.a).unwrap().len(), 2);
    assert!(store::get(&f.app, &f.a, &managed).is_ok());
    assert!(store::update_profile(&f.app, &f.a, &original, &p, Some(&json!([]))).is_err());
    assert_eq!(store::list(&f.app, &f.a).unwrap().len(), 2);
    let ids: Vec<_> = store::list(&f.app, &f.a)
        .unwrap()
        .iter()
        .map(|v| v["id"].as_str().unwrap().to_owned())
        .collect();
    f.app
        .store
        .conn()
        .unwrap()
        .execute("DELETE FROM subscriptions WHERE user_id=?", [&f.a])
        .unwrap();
    store::migrate_legacy(&f.app, &f.a).unwrap();
    assert!(store::list(&f.app, &f.a).unwrap().is_empty());
    assert_eq!(ids.len(), 2);
}
#[tokio::test]
async fn import_is_atomic_and_idempotent() {
    let f = Fixture::new();
    let bad =
        json!({"feeds":[{"url":"https://example.com/feed"},{"url":"http://127.0.0.1/private"}]});
    assert!(
        api::import(State(f.app.clone()), User(f.a.clone()), Json(bad))
            .await
            .is_err()
    );
    assert_eq!(count(&f, "subscriptions"), 0);
    assert_eq!(count(&f, "subscription_sources"), 0);
    let good = json!({"feeds":[{"url":"https://example.com/feed","title":"Example"},{"url":"https://example.com/feed","title":"Duplicate"}]});
    for _ in 0..2 {
        let v = api::import(State(f.app.clone()), User(f.a.clone()), Json(good.clone()))
            .await
            .unwrap();
        assert_eq!(v.0["count"], 1);
    }
    assert_eq!(count(&f, "subscriptions"), 1);
}
#[test]
fn materials_round_robin_sources_and_preserve_first_seen_on_refetch() {
    let f = Fixture::new();
    let at = chrono::Utc::now().timestamp();
    for n in 0..3 {
        add(&f, &f.a, &format!("https://example.com/feed/{n}"));
    }
    while let Some(claim) = worker::claim(&f.app, at).unwrap() {
        let material = (0..200)
            .map(|i| {
                let mut m = article();
                m.url = format!("https://example.com/{}/{i}", claim.id);
                m
            })
            .collect();
        worker::finish(&f.app, &claim, Ok(material), at + 1).unwrap();
    }
    let selected = items::materials(&f.app, &f.a).unwrap();
    assert_eq!(selected.len(), 200);
    let mut counts = std::collections::HashMap::new();
    for m in selected {
        let source = m.url.split('/').nth(3).unwrap().to_owned();
        *counts.entry(source).or_insert(0) += 1;
    }
    assert_eq!(counts.len(), 3);
    assert!(counts.values().all(|n| *n >= 66));
    let claim = worker::claim(&f.app, at + worker::POLL_SECONDS + 2)
        .unwrap()
        .unwrap();
    let before: Vec<(String, i64)> = {
        let conn = f.app.store.conn().unwrap();
        let mut stmt = conn
            .prepare("SELECT id,first_seen FROM subscription_items WHERE source_id=? ORDER BY id")
            .unwrap();
        let rows = stmt
            .query_map([&claim.id], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();
        rows
    };
    let material = (0..200)
        .map(|i| {
            let mut m = article();
            m.url = format!("https://example.com/{}/{i}", claim.id);
            m
        })
        .collect();
    worker::finish(&f.app, &claim, Ok(material), at + worker::POLL_SECONDS + 3).unwrap();
    let conn = f.app.store.conn().unwrap();
    let unchanged: i64 = conn
        .query_row(
            "SELECT count(*) FROM subscription_items WHERE source_id=? AND first_seen=?",
            params![claim.id, at + 1],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(unchanged, before.len() as i64);
}

#[test]
fn leases_enforce_database_wide_and_host_bounds() {
    let f = Fixture::new();
    let at = chrono::Utc::now().timestamp();
    for url in [
        "https://example.com/a",
        "https://example.com/b",
        "https://one.example.org/feed",
        "https://two.example.org/feed",
        "https://three.example.org/feed",
        "https://four.example.org/feed",
    ] {
        add(&f, &f.a, url);
    }
    let mut claims = Vec::new();
    while let Some(c) = worker::claim(&f.app, at).unwrap() {
        claims.push(c);
    }
    assert_eq!(claims.len(), 4);
    let hosts: std::collections::HashSet<_> = claims
        .iter()
        .map(|c| {
            crate::network::validate_url(&c.url)
                .unwrap()
                .host_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    assert_eq!(hosts.len(), 4);
    // A second store connection sees the same leases (as would another server process).
    let reopened = App::open(f.app.data_dir.as_ref().clone()).unwrap();
    assert!(worker::claim(&reopened, at).unwrap().is_none());
    worker::finish(&f.app, &claims[0], Ok(vec![]), at + 1).unwrap();
    assert!(worker::claim(&reopened, at + 2).unwrap().is_some());
}
#[test]
fn startup_migration_survives_reopen_without_resurrecting_legacy_or_exposing_credentials() {
    let f = Fixture::new();
    let mut p = f.app.store.profile(&f.a).unwrap();
    p["feeds"] = json!(["https://example.com/secret-path?token=private"]);
    f.app.store.save_profile(&f.a, &p).unwrap();
    let reopened = App::open(f.app.data_dir.as_ref().clone()).unwrap();
    assert_eq!(store::list(&reopened, &f.a).unwrap().len(), 1);
    assert_eq!(reopened.store.profile(&f.a).unwrap()["feeds"], json!([]));
    assert!(store::legacy_feeds(&reopened, &f.a).unwrap().is_empty());
    store::clear_user(&reopened.store.conn().unwrap(), &f.a).unwrap();
    drop(reopened);
    let reopened = App::open(f.app.data_dir.as_ref().clone()).unwrap();
    assert!(store::list(&reopened, &f.a).unwrap().is_empty());
}

#[path = "subscription_order_tests.rs"]
mod ordering;
