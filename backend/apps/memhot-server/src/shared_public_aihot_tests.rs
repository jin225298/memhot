use super::*;
fn at() -> DateTime<Utc> {
    timestamp("2026-10-13T02:00:00Z").unwrap()
}
fn item(n: usize) -> Value {
    json!({"title":format!("报道 {n}"),"summary":"原刊完整摘要。".repeat(300),
        "source":{"name":format!("来源 {n}")},"links":{"original":format!("https://example.org/article-{n}")},
        "attribution":{"name":"AIHOT","url":format!("https://aihot.news/items/item-{n}")}})
}
fn fixture() -> Value {
    json!({"date":"2026-10-13","generatedAt":"2026-10-13T00:00:19Z",
        "windowStart":"2026-10-12T00:00:00Z","windowEnd":"2026-10-13T00:00:00Z",
        "attribution":{"name":"AIHOT","url":"https://aihot.news/daily/2026-10-13"},
        "lead":{"title":"报道 1","leadParagraph":"来自原刊的导语"},
        "sections":[{"label":"模型","items":[item(1)]},{"label":"行业","items":[item(2)]}],
        "flashes":[{"title":"快讯 3","source":{"name":"来源 3"},
            "links":{"original":"https://example.org/flash"},
            "publishedAt":"2026-10-13T00:00:19Z",
            "attribution":{"name":"AIHOT","url":"https://aihot.news/items/flash"}}]})
}
#[test]
fn whole_daily_keeps_sections_flashes_dates_and_attribution_without_claiming_verification() {
    let raw = fixture();
    let report = decode(&raw, at()).unwrap();
    assert_eq!(report["editorial"]["stories"].as_array().unwrap().len(), 2);
    assert_eq!(report["editorial"]["flashes"].as_array().unwrap().len(), 1);
    assert_eq!(report["editorial"]["sections"].as_array().unwrap().len(), 2);
    assert_eq!(
        report["editorial"]["stories"][0]["summary"],
        raw["sections"][0]["items"][0]["summary"]
    );
    assert_eq!(report["summary"], "来自原刊的导语");
    assert_eq!(report["createdAt"], raw["generatedAt"]);
    assert_eq!(report["editorial"]["citationReview"], "external_editorial");
    assert_eq!(report["editorial"]["highlights"], json!([]));
    for source in report["sources"].as_array().unwrap() {
        assert_eq!(source["retrievalKind"], "search_snippet");
        assert!(source["publishedAt"].is_null());
        assert_eq!(source["attribution"]["name"], "AIHOT");
        assert_eq!(source["citations"], json!([]));
    }
}
#[test]
fn stale_issue_remains_old_and_quiet_issue_is_distinct_from_failed_retrieval() {
    let mut raw = fixture();
    let stale = decode(&raw, at() + Duration::days(1)).unwrap();
    assert_eq!(stale["status"], "partial");
    assert_eq!(stale["createdAt"], raw["generatedAt"]);
    assert!(!current(&stale, at() + Duration::days(1)));
    raw["sections"] = json!([]);
    raw["flashes"] = json!([]);
    let quiet = decode(&raw, at()).unwrap();
    assert_eq!(quiet["editorial"]["quietIssue"], true);
    assert!(!crate::reading::incomplete(&quiet));
    raw["lead"] = Value::Null;
    assert!(decode(&raw, at()).is_err());
    assert!(decode(&Value::Null, at()).is_err());
}
#[test]
fn issue_freshness_crosses_cutoff_even_when_the_cached_status_was_ready() {
    let report = decode(&fixture(), at()).unwrap();
    assert!(!is_stale(
        &report,
        timestamp("2026-10-13T23:59:59Z").unwrap()
    ));
    assert!(is_stale(
        &report,
        timestamp("2026-10-14T00:00:00Z").unwrap()
    ));
}
#[test]
fn malformed_or_unsafe_issue_never_partially_imports() {
    for (pointer, value) in [
        ("/attribution/name", json!("someone else")),
        ("/attribution/url", json!("https://evil.example/daily")),
        ("/date", json!("2026-10-14")),
        ("/windowEnd", json!("2026-10-14T00:00:00Z")),
        ("/generatedAt", json!("2026-10-13T03:00:00Z")),
        ("/windowStart", json!("2026-10-13T01:00:00Z")),
        (
            "/sections/0/items/0/links/original",
            json!("http://127.0.0.1/secret"),
        ),
        (
            "/sections/0/items/0/links/original",
            json!("javascript:alert(1)"),
        ),
        ("/sections/0/items/0/summary", json!("x".repeat(6001))),
        (
            "/sections/0/items/0/attribution/url",
            json!("https://aihot.news/?key=private"),
        ),
    ] {
        let mut raw = fixture();
        *raw.pointer_mut(pointer).unwrap() = value;
        assert!(decode(&raw, at()).is_err(), "{pointer}");
    }
    let mut raw = fixture();
    raw["flashes"] = json!([item(1)]);
    assert!(decode(&raw, at()).is_err());
}
#[test]
fn one_shared_issue_hydrates_only_owned_references_without_model_billing() {
    let directory = std::env::temp_dir().join(format!("memhot-syndicated-{}", crate::id()));
    let app = App::open(directory.clone()).unwrap();
    let report = decode(&fixture(), at()).unwrap();
    let key = report["id"].as_str().unwrap();
    let lease = match acquire(&app, key, "edition").unwrap() {
        Reservation::Refresh(l) => l,
        _ => panic!(),
    };
    assert!(commit(&app, &lease, &report, 300, 30 * 86400).unwrap());
    let (owner, _) = crate::auth::create_pair(&app, "owner", None).unwrap();
    let (other, _) = crate::auth::create_pair(&app, "other", None).unwrap();
    app.store.put(&owner,"report",&json!({"id":"ref","kind":"digest","publicEditionId":key,"createdAt":report["createdAt"]}),None).unwrap();
    assert_eq!(
        crate::report_access::get(&app, &owner, "ref").unwrap()["editorial"]["metrics"]
            ["storyCount"],
        3
    );
    assert!(crate::report_access::get(&app, &other, "ref").is_err());
    assert_eq!(
        app.store
            .conn()
            .unwrap()
            .query_row("SELECT count(*) FROM public_model_calls", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    drop(app);
    std::fs::remove_dir_all(directory).unwrap();
}
