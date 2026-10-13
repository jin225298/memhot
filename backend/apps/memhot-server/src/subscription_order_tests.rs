use super::{add, article, count, items, store, worker, Fixture};
use crate::auth::User;
use axum::{
    extract::{Path, State},
    Json,
};

#[tokio::test]
async fn initial_backfill_ranks_original_dates_before_hashes_and_queries_before_cap() {
    let f = Fixture::new();
    let at = chrono::Utc::now().timestamp();
    for source in crate::subscription_catalog::entries().iter().take(2) {
        store::add(&f.app, &f.a, source.url, None, None, Some(source.id)).unwrap();
    }
    let mut fresh_urls = Vec::new();
    let mut buried_urls = Vec::new();
    while let Some(claim) = worker::claim(&f.app, at).unwrap() {
        let mut urls: Vec<_> = (0..200)
            .map(|n| format!("https://example.com/{}/{n}", claim.id))
            .collect();
        urls.sort_by_key(|u| crate::hash(u.as_bytes()));
        let fresh = urls[199].clone();
        let buried = urls[198].clone();
        let batch = urls
            .into_iter()
            .map(|url| {
                let mut m = article();
                m.url = url.clone();
                if url == fresh {
                    m.title = "Newest original release".into();
                    m.published = Some(chrono::Utc::now().to_rfc3339());
                } else if url == buried {
                    m.title = "Ceramics archival study".into();
                    m.published = Some("Mon, 01 Jan 2001 12:00:00 +0000".into());
                }
                m
            })
            .collect();
        worker::finish(&f.app, &claim, Ok(batch), at + 1).unwrap();
        fresh_urls.push(fresh);
        buried_urls.push(buried);
    }
    assert_eq!(count(&f, "subscription_items"), 400);
    for selected in [
        items::materials(&f.app, &f.a).unwrap(),
        items::public_materials(&f.app).unwrap(),
    ] {
        assert_eq!(selected.len(), 200);
        for url in &fresh_urls {
            assert!(
                selected.iter().any(|m| m.url == *url),
                "fresh release was excluded by URL hash order"
            );
        }
        for url in &buried_urls {
            assert!(!selected.iter().any(|m| m.url == *url));
        }
    }
    let found = items::search_materials_page(&f.app, &f.a, "ceramics", 0)
        .unwrap()
        .0;
    assert_eq!(found.len(), 2);
    assert!(found.iter().all(|m| buried_urls.contains(&m.url)));
    assert!(items::search_materials_page(&f.app, &f.b, "ceramics", 0)
        .unwrap()
        .0
        .is_empty());
    for source in store::list(&f.app, &f.a).unwrap() {
        let id = source["id"].as_str().unwrap().to_owned();
        let Json(detail) = items::list(State(f.app.clone()), User(f.a.clone()), Path(id))
            .await
            .unwrap();
        assert_eq!(detail["items"].as_array().unwrap().len(), 100);
        assert!(fresh_urls
            .iter()
            .any(|url| detail["items"][0]["url"] == *url));
    }
}

#[test]
fn eviction_keeps_new_publications_even_when_backfilled_old_items_arrive_later() {
    let f = Fixture::new();
    add(&f, &f.a, "https://example.com/feed");
    let at = chrono::Utc::now().timestamp();
    for batch in 0..6 {
        let claim = worker::claim(&f.app, at + batch * (worker::POLL_SECONDS + 2))
            .unwrap()
            .unwrap();
        let articles = (0..200)
            .map(|n| {
                let mut m = article();
                m.url = format!("https://example.com/{batch}/{n}");
                if batch == 0 && n == 0 {
                    m.title = "Newest retained publication".into();
                    m.published = Some(chrono::Utc::now().to_rfc3339());
                }
                m
            })
            .collect();
        worker::finish(
            &f.app,
            &claim,
            Ok(articles),
            at + batch * (worker::POLL_SECONDS + 2) + 1,
        )
        .unwrap();
    }
    assert_eq!(count(&f, "subscription_items"), 1000);
    assert!(items::materials(&f.app, &f.a)
        .unwrap()
        .iter()
        .any(|m| m.title == "Newest retained publication"));
}

#[tokio::test]
async fn normalizes_dates_and_uses_aggregator_discovery_without_inventing_publication() {
    let f = Fixture::new();
    let id = add(&f, &f.a, "https://example.com/feed");
    let at = chrono::Utc::now().timestamp();
    let mut invalid = article();
    invalid.url = "https://example.com/invalid".into();
    invalid.published = Some("not a publication date".into());
    invalid.discovered = Some("invalid date".into());
    let mut aggregator = article();
    aggregator.url = "https://example.com/aggregation".into();
    aggregator.published = None;
    aggregator.publication_source = "aggregator_submission";
    aggregator.discovered = Some(chrono::Utc::now().to_rfc2822());
    let claim = worker::claim(&f.app, at).unwrap().unwrap();
    worker::finish(
        &f.app,
        &claim,
        Ok(vec![invalid, article(), aggregator]),
        at + 1,
    )
    .unwrap();
    let Json(detail) = items::list(State(f.app.clone()), User(f.a.clone()), Path(id))
        .await
        .unwrap();
    assert_eq!(detail["items"][0]["url"], "https://example.com/aggregation");
    for item in detail["items"].as_array().unwrap() {
        assert!(
            chrono::DateTime::parse_from_rfc3339(item["discoveredAt"].as_str().unwrap()).is_ok()
        );
        if let Some(date) = item["publishedAt"].as_str() {
            assert!(chrono::DateTime::parse_from_rfc3339(date).is_ok());
        }
    }
    let invalid = detail["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["url"] == "https://example.com/invalid")
        .unwrap();
    assert!(invalid["publishedAt"].is_null());
    assert_eq!(
        invalid["discoveredAt"],
        store::timestamp(Some(at + 1)).unwrap()
    );
}

#[test]
fn query_pages_reach_every_match_without_overlap_or_cross_tenant_results() {
    let f = Fixture::new();
    let at = chrono::Utc::now().timestamp();
    for n in 0..3 {
        add(&f, &f.a, &format!("https://example.com/feed/{n}"));
    }
    while let Some(claim) = worker::claim(&f.app, at).unwrap() {
        let entries = (0..150)
            .map(|n| {
                let mut item = article();
                item.url = format!("https://example.com/{}/{n}", claim.id);
                item
            })
            .collect();
        worker::finish(&f.app, &claim, Ok(entries), at + 1).unwrap();
    }
    let mut all = std::collections::HashSet::new();
    for (offset, expected, more) in [
        (0, 200, true),
        (200, 200, true),
        (400, 50, false),
        (600, 0, false),
    ] {
        let (page, has_more) =
            items::search_materials_page(&f.app, &f.a, "historical", offset).unwrap();
        assert_eq!(page.len(), expected);
        assert_eq!(has_more, more);
        for item in page {
            assert!(all.insert(item.url), "search page repeated a prior match");
        }
    }
    assert_eq!(all.len(), 450);
    let (page, has_more) = items::search_materials_page(&f.app, &f.b, "historical", 0).unwrap();
    assert!(page.is_empty());
    assert!(!has_more);
}
