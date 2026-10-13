mod common;
use axum::http::StatusCode;
use common::{request, user, Fixture};
use memhot_server::{now, reading, App};
use serde_json::{json, Value};
use std::collections::HashSet;

fn put(app: &App, user: &str, kind: &str, value: Value) {
    app.store.put(user, kind, &value, None).unwrap();
}
fn report(id: &str, created: &str, title: &str, category: &str) -> Value {
    json!({"id":id,"kind":"digest","createdAt":created,"title":title,"summary":"可核对的概览","body":"authorized fulltext needle","status":"ready","catalogTopicId":"ai-products","sources":[{"id":"e1","url":"https://example.com/story","title":"原始资料","publisher":"Example","excerpt":"保留的短引文","retrievalKind":"page"}],"editorial":{"version":"public-editorial-v3","stories":[{"id":"s1","eventKey":"event-1","title":title,"summary":"已核对事件摘要","category":category,"evidenceIds":["e1"]}],"flashes":[]}})
}

#[tokio::test]
async fn history_pages_every_owned_delivered_record_with_stable_cursor_and_fulltext_search() {
    let f = Fixture::new();
    let app = &f.app;
    let (owner, token) = user(app, "A").await;
    let (other, other_token) = user(app, "B").await;
    let created = now();
    for i in 0..73 {
        put(
            app,
            &owner,
            "report",
            report(&format!("r{i:03}"), &created, &format!("标题{i}"), "产品"),
        );
    }
    let mut future = report("future", &created, "不能提前读取", "研究");
    future["availableAt"] = json!((chrono::Utc::now() + chrono::Duration::days(1)).to_rfc3339());
    put(app, &owner, "report", future);
    let mut foreign = report("foreign", &created, "外国报告", "研究");
    foreign["body"] = json!("foreign-super-secret");
    put(app, &other, "report", foreign);
    let mut path = "/api/v1/reading?limit=20".to_owned();
    let mut ids = Vec::new();
    let mut first_cursor = None;
    loop {
        let (status, body) = request(app, "GET", &path, &token, None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["overview"]["reportCount"], 73);
        assert_eq!(body["overview"]["storyCount"], 73);
        assert_eq!(body["categories"], json!(["产品"]));
        for item in body["reports"].as_array().unwrap() {
            assert_eq!(item["saved"], false);
            assert_eq!(item["body"], "");
            assert_eq!(item["sources"], json!([]));
            ids.push(item["id"].as_str().unwrap().to_owned());
        }
        let Some(cursor) = body["nextCursor"].as_str() else {
            break;
        };
        if first_cursor.is_none() {
            first_cursor = Some(cursor.to_owned());
        }
        path = format!("/api/v1/reading?limit=20&cursor={cursor}");
    }
    assert_eq!(ids.len(), 73);
    assert_eq!(ids.iter().collect::<HashSet<_>>().len(), 73);
    assert_eq!(ids.first().unwrap(), "r072");
    assert_eq!(ids.last().unwrap(), "r000");
    assert_eq!(
        request(app, "GET", "/api/v1/reading?q=needle", &token, None)
            .await
            .1["overview"]["reportCount"],
        73
    );
    assert_eq!(
        request(
            app,
            "GET",
            "/api/v1/reading?q=foreign-super-secret",
            &token,
            None
        )
        .await
        .1["reports"],
        json!([])
    );
    for (token, suffix) in [(&token, "&q=changed"), (&other_token, "")] {
        let path = format!(
            "/api/v1/reading?cursor={}{}",
            first_cursor.as_ref().unwrap(),
            suffix
        );
        assert_eq!(
            request(app, "GET", &path, token, None).await.0,
            StatusCode::BAD_REQUEST
        );
    }
}

#[tokio::test]
async fn period_kind_category_bookmark_and_legacy_status_are_honest() {
    let f = Fixture::new();
    let app = &f.app;
    let (owner, token) = user(app, "A").await;
    let created = now();
    put(
        app,
        &owner,
        "report",
        report("new", &created, "最近产品", "产品"),
    );
    let old = (chrono::Utc::now() - chrono::Duration::days(10)).to_rfc3339();
    put(
        app,
        &owner,
        "report",
        report("older", &old, "上周研究", "研究"),
    );
    let ancient = (chrono::Utc::now() - chrono::Duration::days(40)).to_rfc3339();
    put(
        app,
        &owner,
        "report",
        report("ancient", &ancient, "旧产品", "产品"),
    );
    put(
        app,
        &owner,
        "report",
        json!({"id":"legacy","kind":"digest","createdAt":created,"title":"旧摘录","status":"ready","sources":[]}),
    );
    put(
        app,
        &owner,
        "report",
        json!({"id":"private","kind":"research","createdAt":created,"title":"个人研究","summary":"研究正文","status":"partial","sources":[]}),
    );
    let week = request(app, "GET", "/api/v1/reading?period=week", &token, None)
        .await
        .1;
    assert_eq!(week["overview"]["reportCount"], 2);
    assert_eq!(week["overview"]["storyCount"], 1);
    assert!(week["overview"]["summary"]
        .as_str()
        .unwrap()
        .contains("最近产品"));
    assert!(!week["overview"]["summary"]
        .as_str()
        .unwrap()
        .contains("旧摘录"));
    assert!(week["overview"]["summary"]
        .as_str()
        .unwrap()
        .contains("1份未完成历史记录"));
    assert_eq!(
        week["reports"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"] == "legacy")
            .unwrap()["status"],
        "incomplete"
    );
    let month = request(
        app,
        "GET",
        "/api/v1/reading?period=month&kind=digest",
        &token,
        None,
    )
    .await
    .1;
    assert_eq!(month["overview"]["reportCount"], 2);
    assert_eq!(month["overview"]["storyCount"], 2);
    assert_eq!(
        request(app, "GET", "/api/v1/reading?kind=research", &token, None)
            .await
            .1["reports"][0]["id"],
        "private"
    );
    assert_eq!(
        request(
            app,
            "GET",
            "/api/v1/reading?category=%E7%A0%94%E7%A9%B6",
            &token,
            None
        )
        .await
        .1["reports"][0]["id"],
        "older"
    );
    for suffix in [
        "period=day",
        "kind=arbitrary",
        "limit=0",
        "limit=51",
        "cursor=garbage",
    ] {
        assert_eq!(
            request(
                app,
                "GET",
                &format!("/api/v1/reading?{suffix}"),
                &token,
                None
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
    }
}

#[tokio::test]
async fn bookmarks_are_idempotent_tenant_scoped_and_cannot_outlive_authorization() {
    let f = Fixture::new();
    let app = &f.app;
    let (a, ta) = user(app, "A").await;
    let (b, tb) = user(app, "B").await;
    put(app, &a, "report", report("same", &now(), "A报告", "产品"));
    for _ in 0..2 {
        let (status, value) = request(
            app,
            "PUT",
            "/api/v1/reports/same/saved",
            &ta,
            Some(json!({"saved":true})),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(value["saved"], true);
    }
    assert_eq!(app.store.list_all(&a, "report_saved").unwrap().len(), 1);
    assert!(reading::saved(app, &a, "same").unwrap());
    assert_eq!(
        request(
            app,
            "PUT",
            "/api/v1/reports/same/saved",
            &tb,
            Some(json!({"saved":true}))
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    put(app, &b, "report", report("same", &now(), "B报告", "研究"));
    assert!(!reading::saved(app, &b, "same").unwrap());
    assert_eq!(
        request(app, "GET", "/api/v1/reading?saved=true", &tb, None)
            .await
            .1["reports"],
        json!([])
    );
    assert_eq!(
        request(app, "GET", "/api/v1/reading?saved=true", &ta, None)
            .await
            .1["reports"][0]["saved"],
        true
    );
    for _ in 0..2 {
        assert_eq!(
            request(
                app,
                "PUT",
                "/api/v1/reports/same/saved",
                &ta,
                Some(json!({"saved":false}))
            )
            .await
            .0,
            StatusCode::OK
        );
    }
    assert!(!reading::saved(app, &a, "same").unwrap());
    request(
        app,
        "PUT",
        "/api/v1/reports/same/saved",
        &ta,
        Some(json!({"saved":true})),
    )
    .await;
    assert_eq!(
        request(app, "DELETE", "/api/v1/reports/same", &ta, None)
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        reading::saved(app, &a, "same").unwrap_err().status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        request(app, "GET", "/api/v1/reading?saved=true", &ta, None)
            .await
            .1["reports"],
        json!([])
    );
    assert_eq!(
        request(
            app,
            "PUT",
            "/api/v1/reports/same/saved",
            &ta,
            Some(json!({"saved":true}))
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn event_history_requires_real_identity_or_shared_sources_for_same_event() {
    let f = Fixture::new();
    let app = &f.app;
    let (a, ta) = user(app, "A").await;
    let (b, tb) = user(app, "B").await;
    let created = now();
    put(
        app,
        &a,
        "report",
        report("current", &created, "当前事件", "产品"),
    );
    let mut by_key = report("by-key", &created, "事件更新", "产品");
    by_key["sources"][0]["url"] = json!("https://different.example.com/key");
    put(app, &a, "report", by_key);
    let mut by_url = report("by-url", &created, "共同来源", "产品");
    by_url["editorial"]["stories"][0]["eventKey"] = json!("different-key");
    put(app, &a, "report", by_url);
    let mut category = report("category", &created, "同类别消息", "产品");
    category["sources"][0]["url"] = json!("https://different.example.com/category");
    category["editorial"]["stories"][0]["eventKey"] = json!("another-event");
    put(app, &a, "report", category.clone());
    category["id"] = json!("other-topic");
    category["catalogTopicId"] = json!("mathematics");
    put(app, &a, "report", category);
    put(
        app,
        &b,
        "report",
        report("foreign", &created, "无权看到的更新", "产品"),
    );
    let mut future = report("future", &created, "未来事件", "产品");
    future["availableAt"] = json!((chrono::Utc::now() + chrono::Duration::days(1)).to_rfc3339());
    put(app, &a, "report", future);
    let (status, value) = request(app, "GET", "/api/v1/reports/current/events/s1", &ta, None).await;
    assert_eq!(status, StatusCode::OK);
    let items = value["related"].as_array().unwrap();
    assert_eq!(items.len(), 3);
    for (id, relationship) in [
        ("by-key", "same_event"),
        ("by-url", "same_event"),
        ("category", "related"),
    ] {
        assert_eq!(
            items.iter().find(|r| r["reportId"] == id).unwrap()["relationship"],
            relationship
        );
    }
    assert_eq!(
        request(app, "GET", "/api/v1/reports/current/events/s1", &tb, None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        request(
            app,
            "GET",
            "/api/v1/reports/current/events/missing",
            &ta,
            None
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn historical_zero_claim_research_is_visible_but_excluded_from_delivery_overview() {
    let f = Fixture::new();
    let app = &f.app;
    let (a, ta) = user(app, "A").await;
    let created = chrono::Utc::now().to_rfc3339();
    for (id, title, claims) in [
        ("unverified", "未经核对的搜索清单", json!([])),
        (
            "verified",
            "完成核对的研究",
            json!([{"text":"有引用的研究结论","evidenceIds":["e"]}]),
        ),
    ] {
        put(
            app,
            &a,
            "report",
            json!({"id":id,"kind":"research","title":title,"status":"partial",
                "createdAt":created,"claims":claims,"sources":[],"body":"研究历史内容"}),
        );
    }
    let (status, value) = request(app, "GET", "/api/v1/reading?kind=research", &ta, None).await;
    assert_eq!(status, StatusCode::OK);
    let reports = value["reports"].as_array().unwrap();
    assert_eq!(reports.len(), 2);
    assert_eq!(
        reports.iter().find(|r| r["id"] == "unverified").unwrap()["status"],
        "incomplete"
    );
    assert_eq!(value["overview"]["reportCount"], 1);
    let summary = value["overview"]["summary"].as_str().unwrap();
    assert!(!summary.contains("未经核对的搜索清单"));
    assert!(summary.contains("完成核对的研究"));
    assert!(summary.contains("另保留1份未完成历史记录"));
}

#[tokio::test]
async fn pagination_orders_submillisecond_and_offset_timestamps_without_losing_records() {
    let f = Fixture::new();
    let app = &f.app;
    let (a, ta) = user(app, "A").await;
    let second = chrono::Utc::now();
    let prefix = second.format("%Y-%m-%dT%H:%M:%S");
    let offset = (second + chrono::Duration::hours(8))
        .format("%Y-%m-%dT%H:%M:%S")
        .to_string();
    for (id, date) in [
        ("a", format!("{prefix}.000000002Z")),
        ("z", format!("{prefix}.000000001Z")),
        ("m", format!("{offset}.000000001+08:00")),
    ] {
        put(app, &a, "report", report(id, &date, id, "产品"));
    }
    let mut path = "/api/v1/reading?limit=1".to_owned();
    let mut found = Vec::new();
    loop {
        let (status, value) = request(app, "GET", &path, &ta, None).await;
        assert_eq!(status, StatusCode::OK);
        found.extend(
            value["reports"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| r["id"].as_str().unwrap().to_owned()),
        );
        let Some(cursor) = value["nextCursor"].as_str() else {
            break;
        };
        path = format!("/api/v1/reading?limit=1&cursor={cursor}");
    }
    assert_eq!(found, vec!["a", "z", "m"]);
}
