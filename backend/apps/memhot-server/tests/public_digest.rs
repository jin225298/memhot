mod common;
use chrono::Timelike;
use common::*;
use memhot_server::{digest, hash, now, report_access, shared_public, worker, App};
use serde_json::{json, Value};

fn seed(app: &App, topic: &str) -> String {
    let at = chrono::Utc::now();
    let date = at.date_naive().to_string();
    let window = at
        .date_naive()
        .and_hms_opt(at.hour() / 6 * 6, 0, 0)
        .unwrap()
        .and_utc()
        .to_rfc3339();
    let key = format!(
        "edition-{}",
        hash(format!("{topic}:{window}:zh-CN:{}", shared_public::PIPELINE_VERSION).as_bytes())
    );
    let value = json!({"id":key,"title":"公开测试日报","summary":"经过整理的公开材料","body":"公开资料的合成正文，用于确认引用恢复。","kind":"digest","status":"ready","sources":[{"id":"public-source","url":"https://example.com/public","title":"公开原文","excerpt":"公开摘要"}],"gaps":[],"createdAt":now(),"utcDate":date,"catalogTopicId":topic,"language":"zh-CN","pipelineVersion":shared_public::PIPELINE_VERSION});
    let at = chrono::Utc::now().timestamp();
    app.store.conn().unwrap().execute("INSERT INTO public_cache(cache_key,kind,body,expires_at,retain_until) VALUES(?,'edition',?,?,?)",rusqlite::params![key,value.to_string(),at+3600,at+86400]).unwrap();
    key
}
async fn interests(app: &App, token: &str, values: Value) {
    assert_eq!(
        request(
            app,
            "PUT",
            "/api/v1/profile",
            token,
            Some(json!({"interests":values}))
        )
        .await
        .0,
        200
    );
}
async fn public_job(app: &App, user: &str, key: &str, available: Option<String>) -> String {
    let mut payload = json!({"kind":"digest","digestStage":"public","catalogTopicId":"ai"});
    if let Some(at) = available {
        payload["availableAt"] = json!(at);
    }
    app.store.queue(user, "digest", payload, key).unwrap();
    let job = app.store.claim().unwrap().unwrap();
    assert_eq!(job.user, user);
    worker::execute(app, &job).await.unwrap();
    app.store.list(user, "report").unwrap()[0]["id"]
        .as_str()
        .unwrap()
        .to_owned()
}
#[tokio::test]
async fn two_users_share_one_edition_but_keep_owned_refs_and_no_user_charge() {
    let f = Fixture::new();
    let app = &f.app;
    let (a, ta) = user(app, "A").await;
    let (b, tb) = user(app, "B").await;
    interests(app, &ta, json!(["AI"])).await;
    interests(app, &tb, json!(["人工智能"])).await;
    let edition = seed(app, "ai");
    memhot_server::billing::ensure_subscription(app, &a).unwrap();
    memhot_server::billing::ensure_subscription(app, &b).unwrap();
    app.store
        .conn()
        .unwrap()
        .execute("UPDATE billing_wallets SET included=0,topup=0", [])
        .unwrap();
    let ra = public_job(app, &a, "a", None).await;
    let rb = public_job(app, &b, "b", None).await;
    assert_ne!(ra, rb);
    for (uid, key) in [(&a, &ra), (&b, &rb)] {
        let raw = app.store.get(uid, "report", key).unwrap();
        assert_eq!(raw["publicEditionId"], edition);
        for field in ["body", "sources", "summary", "claims", "editorial"] {
            assert!(raw.get(field).is_none(), "{field}");
        }
        let hydrated = report_access::get(app, uid, key).unwrap();
        assert!(hydrated["body"].as_str().unwrap().contains("合成正文"));
        assert_eq!(hydrated["sources"].as_array().unwrap().len(), 1);
    }
    assert!(report_access::get(app, &b, &ra).is_err());
    assert_eq!(
        request(app, "GET", &format!("/api/v1/reports/{ra}"), &tb, None)
            .await
            .0,
        404
    );
    assert_eq!(public_job(app, &a, "a-refresh", None).await, ra);
    assert_eq!(app.store.list(&a, "report").unwrap().len(), 1);
    let conn = app.store.conn().unwrap();
    let editions: i64 = conn
        .query_row(
            "SELECT count(*) FROM public_cache WHERE kind='edition'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(editions, 1);
    let usage: i64 = conn
        .query_row("SELECT count(*) FROM billing_usage", [], |r| r.get(0))
        .unwrap();
    assert_eq!(usage, 0);
}
#[tokio::test]
async fn scheduled_refs_stay_hidden_and_stale_is_visible_after_delivery() {
    let f = Fixture::new();
    let app = &f.app;
    let (uid, token) = user(app, "A").await;
    interests(app, &token, json!(["AI"])).await;
    let edition = seed(app, "ai");
    let future = (chrono::Utc::now() + chrono::Duration::hours(2)).to_rfc3339();
    let key = public_job(app, &uid, "scheduled", Some(future)).await;
    assert!(report_access::get(app, &uid, &key).is_err());
    assert!(report_access::list(app, &uid).unwrap().is_empty());
    assert!(
        request(app, "GET", "/api/v1/home", &token, None).await.1["reports"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(request(app, "GET", "/api/v1/delivery", &token, None)
        .await
        .1["reports"]
        .as_array()
        .unwrap()
        .is_empty());
    public_job(app, &uid, "explicit-now", None).await;
    app.store
        .conn()
        .unwrap()
        .execute(
            "UPDATE public_cache SET expires_at=0 WHERE cache_key=?",
            [&edition],
        )
        .unwrap();
    let stale = report_access::get(app, &uid, &key).unwrap();
    assert_eq!(stale["cacheStatus"], "stale");
    assert_eq!(stale["status"], "partial");
    assert!(!stale["gaps"].as_array().unwrap().is_empty());
    app.store
        .conn()
        .unwrap()
        .execute(
            "UPDATE public_cache SET retain_until=0 WHERE cache_key=?",
            [&edition],
        )
        .unwrap();
    worker::retention(app).unwrap();
    assert_eq!(
        report_access::get(app, &uid, &key).unwrap()["cacheStatus"],
        "expired"
    );
}
#[tokio::test]
async fn unknown_public_outcome_stays_terminal_while_active_refresh_can_retry() {
    for (error_code, expected, status) in [
        (
            Some("public_outcome_unknown"),
            "public_digest_unavailable",
            "failed",
        ),
        (None, "busy", "queued"),
    ] {
        let f = Fixture::new();
        let app = &f.app;
        let (uid, token) = user(app, "A").await;
        interests(app, &token, json!(["AI"])).await;
        let edition = seed(app, "ai");
        app.store.conn().unwrap().execute(
            "UPDATE public_cache SET body=NULL,expires_at=0,retry_after=?,last_error=? WHERE cache_key=?",
            rusqlite::params![chrono::Utc::now().timestamp()+3600,error_code,edition],
        ).unwrap();
        app.store
            .queue(
                &uid,
                "digest",
                json!({"kind":"digest","digestStage":"public","catalogTopicId":"ai"}),
                "pending",
            )
            .unwrap();
        let job = app.store.claim().unwrap().unwrap();
        let error = worker::execute(app, &job).await.unwrap_err();
        assert_eq!(error.code, expected);
        if error_code.is_some() {
            assert!(error.message.contains("停止自动重放"));
            assert!(!error.message.contains("正在刷新"));
        }
        app.store.fail(&job, &error).unwrap();
        let actual: String = app
            .store
            .conn()
            .unwrap()
            .query_row("SELECT status FROM jobs WHERE id=?", [job.id], |r| r.get(0))
            .unwrap();
        assert_eq!(actual, status);
        assert!(app.store.list(&uid, "report").unwrap().is_empty());
    }
}
#[tokio::test]
async fn fanout_covers_all_catalog_topics_and_keeps_custom_interest_and_feeds_private() {
    let f = Fixture::new();
    let app = &f.app;
    let (uid, token) = user(app, "A").await;
    interests(
        app,
        &token,
        json!(["AI", "机器人", "天文", "物理", "我的保密产品成本"]),
    )
    .await;
    app.store.put(&uid,"cognition",&json!({"id":"inferred","type":"interest","statement":"私有推断兴趣","status":"active","createdAt":now()}),None).unwrap();
    app.store.put(&uid,"cognition",&json!({"id":"inactive","type":"interest","statement":"未确认私密兴趣","status":"pending","createdAt":now()}),None).unwrap();
    assert_eq!(
        request(
            app,
            "PUT",
            "/api/v1/profile",
            &token,
            Some(json!({"feeds":["https://example.com/private-feed.xml"]}))
        )
        .await
        .0,
        200
    );
    let plan = digest::interests(app, &uid).unwrap();
    assert_eq!(
        plan.feeds,
        vec![json!("https://example.com/private-feed.xml")]
    );
    assert_eq!(plan.public.len(), 4);
    assert_eq!(plan.private.len(), 2);
    assert!(!plan.private.iter().any(|s| s.contains("未确认")));
    app.store
        .queue(
            &uid,
            "digest",
            json!({"kind":"digest","availableAt":"2099-01-01T08:00:00+08:00"}),
            "parent",
        )
        .unwrap();
    let parent = app.store.claim().unwrap().unwrap();
    worker::execute(app, &parent).await.unwrap();
    let conn = app.store.conn().unwrap();
    let mut q = conn
        .prepare("SELECT payload FROM jobs WHERE status='queued' AND user_id=?")
        .unwrap();
    let payloads: Vec<Value> = q
        .query_map([&uid], |r| r.get::<_, String>(0))
        .unwrap()
        .map(|r| serde_json::from_str(&r.unwrap()).unwrap())
        .collect();
    assert_eq!(payloads.len(), 5);
    assert_eq!(
        payloads
            .iter()
            .filter(|p| p["digestStage"] == "private")
            .count(),
        1
    );
    for p in payloads {
        assert_eq!(p["availableAt"], "2099-01-01T08:00:00+08:00");
        assert!(!p.to_string().contains("保密"));
    }
    let n: i64 = conn
        .query_row("SELECT count(*) FROM public_cache", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        n, 0,
        "dispatch cannot submit a private query to public cache"
    );
}

#[tokio::test]
async fn deleting_a_public_delivery_does_not_delete_shared_body_or_resurrect_the_card() {
    let f = Fixture::new();
    let app = &f.app;
    let (uid, token) = user(app, "A").await;
    interests(app, &token, json!(["AI"])).await;
    let edition = seed(app, "ai");
    let key = public_job(app, &uid, "first", None).await;
    app.store
        .queue(
            &uid,
            "digest",
            json!({"kind":"digest","digestStage":"public","catalogTopicId":"ai"}),
            "late",
        )
        .unwrap();
    let job = app.store.claim().unwrap().unwrap();
    let late = digest::run(app, &job).await.unwrap();
    assert_eq!(
        request(
            app,
            "DELETE",
            &format!("/api/v1/reports/{key}"),
            &token,
            None
        )
        .await
        .0,
        204
    );
    assert!(shared_public::get(app, &edition).unwrap().is_some());
    app.store.finish(&job, &late, "done").unwrap();
    assert!(app.store.list(&uid, "report").unwrap().is_empty());
    app.store
        .queue(
            &uid,
            "digest",
            json!({"kind":"digest","digestStage":"public","catalogTopicId":"ai"}),
            "refresh",
        )
        .unwrap();
    worker::execute(app, &app.store.claim().unwrap().unwrap())
        .await
        .unwrap();
    assert!(app.store.list(&uid, "report").unwrap().is_empty());
}
