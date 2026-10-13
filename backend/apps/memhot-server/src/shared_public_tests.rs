use super::*;
use crate::{discovery::Material, freshness, now};
use std::collections::HashSet;
struct Fixture {
    app: App,
}
impl Fixture {
    fn new() -> Self {
        Self {
            app: App::open(std::env::temp_dir().join(format!("shared-public-{}", id()))).unwrap(),
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(self.app.data_dir.as_ref());
    }
}
fn refresh(app: &App, key: &str, kind: &str) -> Lease {
    match acquire(app, key, kind).unwrap() {
        Reservation::Refresh(lease) => lease,
        other => panic!("expected lease: {other:?}"),
    }
}
#[test]
fn retention_keeps_failed_unpublished_edition_backoff() {
    let f = Fixture::new();
    let lease = refresh(&f.app, "edition-failed", "edition");
    failed(&f.app, &lease, "public_outcome_unknown").unwrap();
    retention(&f.app).unwrap();
    assert!(matches!(
        acquire(&f.app, "edition-failed", "edition").unwrap(),
        Reservation::Waiting(None)
    ));
    let error: String = f
        .app
        .store
        .conn()
        .unwrap()
        .query_row(
            "SELECT last_error FROM public_cache WHERE cache_key='edition-failed'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(error, "public_outcome_unknown");
}
#[test]
fn only_exact_declared_aliases_enter_public_catalog() {
    let topics = match_topics(&[
        "AI".into(),
        " 人工智能 ".into(),
        "具身智能".into(),
        "我的团队用AI研究私人客户".into(),
    ]);
    assert_eq!(
        topics.iter().map(|t| t.id).collect::<Vec<_>>(),
        ["ai", "robotics"]
    );
    assert!(match_topics(&["我喜欢AI但这是私人研究".into()]).is_empty());
    assert_ne!(
        edition_key("ai", "2026-10-12", "zh-CN"),
        edition_key("ai", "2026-10-13", "zh-CN")
    );
    assert_ne!(
        edition_key("ai", "2026-10-12", "zh-CN"),
        edition_key("ai", "2026-10-12", "en")
    );
}
#[tokio::test]
async fn arbitrary_urls_credentials_and_query_tokens_cannot_enter_shared_cache() {
    let f = Fixture::new();
    for url in [
        "https://user:pass@example.org/page",
        "https://example.org/page?token=secret",
        "https://example.org/page?q=private",
        "https://example.org/page#private",
        "http://example.org/page",
        "https://127.0.0.1/page",
    ] {
        assert!(safe_url(url).is_err(), "{url}");
    }
    assert!(
        cached_page(&f.app, "https://example.org/not-from-public-catalog")
            .await
            .is_err()
    );
    let count: i64 = f
        .app
        .store
        .conn()
        .unwrap()
        .query_row("SELECT count(*) FROM public_cache", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0);
    assert!(edition(&f.app, "my-private-interest", "zh-CN")
        .await
        .is_err());
}
#[test]
fn cache_is_singleflight_across_connections_and_rejects_a_stale_writer() {
    let f = Fixture::new();
    let other = App::open(f.app.data_dir.as_ref().clone()).unwrap();
    let first = refresh(&f.app, "edition-test", "edition");
    assert!(matches!(
        acquire(&other, "edition-test", "edition").unwrap(),
        Reservation::Waiting(None)
    ));
    f.app
        .store
        .conn()
        .unwrap()
        .execute(
            "UPDATE public_cache SET lease_until=0 WHERE cache_key='edition-test'",
            [],
        )
        .unwrap();
    let second = refresh(&other, "edition-test", "edition");
    assert!(!commit(&f.app, &first, &json!({"id":"old"}), 60, 86400).unwrap());
    assert!(commit(&other, &second, &json!({"id":"new","gaps":[]}), 60, 86400).unwrap());
    assert_eq!(get(&f.app, "edition-test").unwrap().unwrap()["id"], "new");
}
#[test]
fn failed_refresh_preserves_explicitly_stale_body_and_sets_backoff() {
    let f = Fixture::new();
    let first = refresh(&f.app, "page-test", "page");
    commit(
        &f.app,
        &first,
        &json!({"text":"public old text"}),
        -1,
        86400,
    )
    .unwrap();
    let next = refresh(&f.app, "page-test", "page");
    assert!(next.previous.is_some());
    failed(&f.app, &next, "network").unwrap();
    let Reservation::Waiting(Some(previous)) = acquire(&f.app, "page-test", "page").unwrap() else {
        panic!("refresh must back off while retaining old content")
    };
    assert_eq!(previous["text"], "public old text");
}
#[test]
fn raw_pages_expire_but_referenced_public_editions_keep_hydratable_quotes() {
    let f = Fixture::new();
    f.app.store.conn().unwrap().execute("INSERT INTO public_model_calls(operation_key,fingerprint,status,response,attempt,created,updated) VALUES('old-response','fingerprint','completed','old model response',1,0,0)",[]).unwrap();
    let user = "reader";
    f.app
        .store
        .conn()
        .unwrap()
        .execute(
            "INSERT INTO users(id,name,profile) VALUES(?,'reader','{}')",
            [user],
        )
        .unwrap();
    let page = refresh(&f.app, "page-old", "page");
    commit(&f.app, &page, &json!({"text":"raw html"}), -1, -1).unwrap();
    let edition = refresh(&f.app, "edition-kept", "edition");
    commit(
        &f.app,
        &edition,
        &json!({"id":"edition-kept","body":"quoted public facts","gaps":[]}),
        -1,
        -1,
    )
    .unwrap();
    f.app
        .store
        .put(
            user,
            "report",
            &json!({"id":"private-report","publicEditionId":"edition-kept","createdAt":now()}),
            None,
        )
        .unwrap();
    retention(&f.app).unwrap();
    let retained_call: (String, Option<String>) = f
        .app
        .store
        .conn()
        .unwrap()
        .query_row(
            "SELECT status,response FROM public_model_calls WHERE operation_key='old-response'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(retained_call, ("completed".into(), None));
    let raw_count: i64 = f
        .app
        .store
        .conn()
        .unwrap()
        .query_row(
            "SELECT count(*) FROM public_cache WHERE kind='page'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(raw_count, 0);
    let hydrated = get(&f.app, "edition-kept").unwrap().unwrap();
    assert_eq!(hydrated["cacheStatus"], "expired");
    assert_eq!(hydrated["body"], "quoted public facts");
}
#[test]
fn source_dates_are_never_inferred_from_fetch_time() {
    let at = Utc::now();
    let make = |published: Option<String>| Material {
        title: "A".into(),
        url: format!("https://example.org/{}", id()),
        text: "public text".into(),
        published,
        discovered: Some(at.to_rfc3339()),
        discovered_via: None,
        publication_source: "unknown",
        publisher: "example.org".into(),
        fulltext: false,
        pdf: None,
    };
    let mut candidates = vec![
        make(None),
        make(Some((at - Duration::days(60)).to_rfc3339())),
        make(Some((at + Duration::days(2)).to_rfc3339())),
        make(Some((at - Duration::hours(2)).to_rfc3339())),
    ];
    assert_eq!(freshness::select(&mut candidates, &HashSet::new(), at), 3);
    assert_eq!(candidates.len(), 1);
}
#[tokio::test]
async fn unknown_public_outcome_is_not_automatically_replayed_or_shown_as_active_work() {
    let f = Fixture::new();
    let key = edition_key("ai", &edition_window(Utc::now()).to_rfc3339(), "zh-CN");
    let lease = refresh(&f.app, &key, "edition");
    failed(&f.app, &lease, "public_outcome_unknown").unwrap();
    let outcome = edition(&f.app, "ai", "zh-CN").await.unwrap();
    assert_eq!(outcome.cache_status, "unavailable");
    assert!(outcome.warning.unwrap().contains("停止自动重放"));
    assert!(matches!(
        acquire(&f.app, &key, "edition").unwrap(),
        Reservation::Waiting(None)
    ));
}
#[test]
fn hydration_reports_expired_freshness_without_losing_saved_body() {
    let f = Fixture::new();
    let lease = refresh(&f.app, "edition-stale", "edition");
    commit(
        &f.app,
        &lease,
        &json!({"body":"verified old body","gaps":[]}),
        -1,
        86400,
    )
    .unwrap();
    let value = get(&f.app, "edition-stale").unwrap().unwrap();
    assert_eq!(value["cacheStatus"], "stale");
    assert_eq!(value["body"], "verified old body");
}

#[tokio::test]
async fn six_hour_windows_refresh_inputs_without_replaying_an_earlier_paid_edition() {
    let f = Fixture::new();
    let seed = |period: &str, label: &str| {
        let key = edition_key("ai", period, "zh-CN");
        let lease = refresh(&f.app, &key, "edition");
        commit(&f.app, &lease, &json!({"id":label,"gaps":[]}), 86400, 86400).unwrap();
        key
    };
    let old = seed("2026-10-12", "old-daily-input");
    let early = seed("2026-10-12T06:00:00+00:00", "morning-input");
    let late = seed("2026-10-12T12:00:00+00:00", "afternoon-input");
    seed("2026-10-12T18:00:00+00:00", "night-input");
    seed("2026-10-13T00:00:00+00:00", "next-day-input");
    for (timestamp, expected) in [
        ("2026-10-12T06:00:00Z", "morning-input"),
        ("2026-10-12T11:59:59Z", "morning-input"),
        ("2026-10-12T12:00:00Z", "afternoon-input"),
        ("2026-10-12T17:59:59Z", "afternoon-input"),
        ("2026-10-12T23:59:59Z", "night-input"),
        ("2026-10-13T00:00:00Z", "next-day-input"),
    ] {
        let at = freshness::publication(timestamp).unwrap();
        let output = edition_at(&f.app, "ai", "zh-CN", at).await.unwrap();
        assert_eq!(output.edition.unwrap()["id"], expected);
        assert_eq!(output.cache_status, "ready");
    }
    assert_ne!(early, late);
    assert_eq!(get(&f.app, &old).unwrap().unwrap()["id"], "old-daily-input");
    assert_eq!(
        f.app
            .store
            .conn()
            .unwrap()
            .query_row("SELECT count(*) FROM public_model_calls", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn no_new_materials_reuses_actual_prior_id_and_date_with_a_distinct_warning() {
    let f = Fixture::new();
    let key = edition_key("ai", &edition_window(Utc::now()).to_rfc3339(), "zh-CN");
    let prior = json!({"id":"prior","catalogTopicId":"ai","language":"zh-CN","pipelineVersion":PIPELINE_VERSION,"utcDate":"2026-10-01","createdAt":"2026-10-01T12:00:00Z","gaps":[]});
    let first = refresh(&f.app, "prior", "edition");
    commit(&f.app, &first, &prior, -1, 86400).unwrap();
    let current = refresh(&f.app, &key, "edition");
    failed(&f.app, &current, "no_new_sources").unwrap();
    let output = edition(&f.app, "ai", "zh-CN").await.unwrap();
    assert_eq!(output.cache_status, "unchanged");
    assert!(output.warning.unwrap().contains("未取得新原文"));
    let edition = output.edition.unwrap();
    assert_eq!(edition["id"], "prior");
    assert_eq!(edition["createdAt"], "2026-10-01T12:00:00Z");
}
