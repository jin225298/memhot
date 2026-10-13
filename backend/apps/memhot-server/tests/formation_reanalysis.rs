mod common;
use common::*;
use memhot_server::{billing, formation, hash, now};
use serde_json::json;
#[tokio::test]
async fn reanalysis_atomically_resets_old_complete_coverage_and_keeps_topics() {
    let f = Fixture::new();
    let (user, token) = user(&f.app, "A").await;
    let source = json!({"id":"source","text":"完整会议","kind":"import","speakerMode":"unknown","status":"ready","analysisStatus":"ready","analysisCoverage":{"includedChars":4,"totalChars":4,"complete":true},"analysisIssues":[{}],"topicIds":["old"],"createdAt":now()});
    f.app.store.put(&user, "source", &source, None).unwrap();
    f.app
        .store
        .put(
            &user,
            "formation_window",
            &json!({"id":"window","sourceId":"source","status":"ready"}),
            None,
        )
        .unwrap();
    f.app
        .store
        .put(
            &user,
            "thought",
            &json!({"id":"old","sourceId":"source"}),
            None,
        )
        .unwrap();
    assert_eq!(
        request(
            &f.app,
            "POST",
            "/api/v1/sources/source/analyze",
            &token,
            None
        )
        .await
        .0,
        200
    );
    let updated = f.app.store.get(&user, "source", "source").unwrap();
    assert_eq!(updated["analysisStatus"], "pending");
    assert_eq!(updated["analysisCoverage"]["includedChars"], 0);
    assert_eq!(updated["analysisCoverage"]["totalChars"], 4);
    assert_eq!(updated["analysisCoverage"]["complete"], false);
    assert_eq!(updated["analysisIssues"], json!([]));
    assert!(f
        .app
        .store
        .list_all(&user, "formation_window")
        .unwrap()
        .is_empty());
    assert!(f.app.store.get(&user, "thought", "old").is_ok());
}
#[tokio::test]
async fn valid_siblings_survive_quote_repair_quota_failure_without_a_network_call() {
    let mut f = Fixture::new();
    let (user, _) = user(&f.app, "A").await;
    f.app.hosted = std::sync::Arc::new(
        json!({"text":{"protocol":"openai","baseUrl":"https://example.com/v1","model":"fixture","apiKey":"fixture"}}),
    );
    billing::ensure_subscription(&f.app, &user).unwrap();
    f.app
        .store
        .conn()
        .unwrap()
        .execute(
            "UPDATE billing_wallets SET included=0,topup=0 WHERE user_id=?",
            [&user],
        )
        .unwrap();
    let text = "第一题；ai第二题";
    let source = json!({"id":"s","text":text,"kind":"import","speakerMode":"unknown","status":"ready","createdAt":now()});
    f.app.store.put(&user, "source", &source, None).unwrap();
    let version = hash(format!("topics-v3:{text}").as_bytes());
    let end = text.chars().count();
    let key = format!("window-{}", hash(format!("s:{version}:0:{end}").as_bytes()));
    let output = json!({"topics":[{"title":"第一题","question":"第一题?","quote":"第一题"},{"title":"第二题","question":"第二题?","quote":"AI第二题"}],"cognitions":[{"type":"interest","statement":"第一题","quote":"第一题","origin":"explicit"}]});
    f.app.store.put(&user,"formation_window",&json!({"id":key,"sourceId":"s","sourceHash":version,"start":0,"end":end,"status":"repairing","output":output}),None).unwrap();
    f.app
        .store
        .queue(&user, "formation", json!({"sourceId":"s"}), "test")
        .unwrap();
    let job = f.app.store.claim().unwrap().unwrap();
    assert_eq!(
        formation::run(&f.app, &job).await.unwrap_err().code,
        "quota_exhausted"
    );
    let topics = f.app.store.list_all(&user, "thought").unwrap();
    assert_eq!(topics.len(), 1);
    assert_eq!(topics[0]["quote"], "第一题");
    assert_eq!(
        f.app.store.list_all(&user, "cognition").unwrap()[0]["status"],
        "pending"
    );
    let source = f.app.store.get(&user, "source", "s").unwrap();
    assert_eq!(source["analysisCoverage"]["complete"], false);
    assert_eq!(source["analysisCoverage"]["scannedChars"], end);
    assert_eq!(source["analysisCoverage"]["rejectedItems"], 1);
    // Retrying uses the same immutable extraction and still cannot charge or duplicate siblings.
    assert_eq!(
        formation::run(&f.app, &job).await.unwrap_err().code,
        "quota_exhausted"
    );
    assert_eq!(f.app.store.list_all(&user, "thought").unwrap().len(), 1);
    let calls: i64 = f
        .app
        .store
        .conn()
        .unwrap()
        .query_row("SELECT count(*) FROM billing_usage", [], |r| r.get(0))
        .unwrap();
    assert_eq!(calls, 0);
}
