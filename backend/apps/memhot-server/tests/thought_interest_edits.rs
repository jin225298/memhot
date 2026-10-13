mod common;
use common::*;
use memhot_server::{cognition, formation, hash, now};
use serde_json::json;

#[tokio::test]
async fn optional_background_can_be_empty_but_required_content_stays_validated() {
    let f = Fixture::new();
    let (uid, token) = user(&f.app, "A").await;
    let (code, thought) = request(
        &f.app,
        "POST",
        "/api/v1/thoughts",
        &token,
        Some(json!({"title":"主题","question":"我想弄清楚的问题","summary":"","category":""})),
    )
    .await;
    assert_eq!(code, 200);
    assert_eq!(thought["summary"], "");
    let key = thought["id"].as_str().unwrap();
    assert_eq!(
        request(
            &f.app,
            "PUT",
            &format!("/api/v1/thoughts/{key}"),
            &token,
            Some(json!({"summary":"先补充背景","revision":1}))
        )
        .await
        .0,
        200
    );
    let (code, cleared) = request(
        &f.app,
        "PUT",
        &format!("/api/v1/thoughts/{key}"),
        &token,
        Some(json!({"summary":"","revision":2})),
    )
    .await;
    assert_eq!(code, 200);
    assert_eq!(cleared["summary"], "");
    assert_eq!(f.app.store.list_all(&uid, "cognition").unwrap().len(), 1);
    for input in [
        json!({"title":"","question":"问题"}),
        json!({"question":""}),
        json!({"question":"问题","summary":null}),
        json!({"question":"问题","summary":"字".repeat(3001)}),
    ] {
        assert_eq!(
            request(&f.app, "POST", "/api/v1/thoughts", &token, Some(input))
                .await
                .0,
            400
        );
    }
}

#[tokio::test]
async fn editing_transcript_content_confirms_interest_without_claiming_the_speaker_is_user() {
    let f = Fixture::new();
    let (uid, token) = user(&f.app, "A").await;
    f.app.store.put(&uid,"thought",&json!({"id":"recorded","title":"会议主题","question":"会议提出的问题？",
        "summary":"某人的想法","category":"未分类","origin":"unverified_transcript","status":"open",
        "analysisStatus":"ready","evidenceRefs":[{"speaker":"unknown"}],"revision":1,"createdAt":now()}),None).unwrap();
    for (input, revision) in [
        (json!({"category":"工作"}), 2),
        (json!({"status":"resolved"}), 3),
        (
            json!({"title":"会议主题","question":"会议提出的问题？","summary":"某人的想法"}),
            4,
        ),
    ] {
        let (code, t) = request(
            &f.app,
            "PUT",
            "/api/v1/thoughts/recorded",
            &token,
            Some(input),
        )
        .await;
        assert_eq!(code, 200);
        assert_eq!(t["revision"], revision);
        assert!(
            f.app.store.list_all(&uid, "cognition").unwrap().is_empty(),
            "classification, status, or unchanged text cannot establish an explicit interest"
        );
    }
    let (code, t) = request(
        &f.app,
        "PUT",
        "/api/v1/thoughts/recorded",
        &token,
        Some(json!({"title":"我想研究这个会议问题","revision":4})),
    )
    .await;
    assert_eq!(code, 200);
    assert_eq!(t["origin"], "unverified_transcript");
    assert_eq!(t["evidenceRefs"][0]["speaker"], "unknown");
    let interest = f
        .app
        .store
        .get(&uid, "cognition", "thought-interest:recorded")
        .unwrap();
    assert_eq!(interest["type"], "interest");
    assert_eq!(interest["origin"], "explicit");
    assert_eq!(interest["evidenceRefs"][0]["quote"], "我想研究这个会议问题");
    assert_eq!(
        interest["evidenceRefs"][0]["userInput"],
        json!({"title":"我想研究这个会议问题"})
    );
    assert!(interest["inferenceReason"]
        .as_str()
        .unwrap()
        .contains("不代表观点或信念"));
    assert_eq!(
        request(
            &f.app,
            "PUT",
            "/api/v1/cognition/thought-interest:recorded",
            &token,
            Some(json!({"statement":"我只想核对会议引用的证据","status":"dismissed","revision":1}))
        )
        .await
        .0,
        200
    );
    assert_eq!(
        request(
            &f.app,
            "PUT",
            "/api/v1/thoughts/recorded",
            &token,
            Some(json!({"question":"另一个会议问题","revision":5}))
        )
        .await
        .0,
        200
    );
    let owned = f
        .app
        .store
        .get(&uid, "cognition", "thought-interest:recorded")
        .unwrap();
    assert_eq!(owned["statement"], "我只想核对会议引用的证据");
    assert_eq!(owned["status"], "dismissed");
}

#[tokio::test]
async fn formation_only_receives_current_active_minimal_tenant_preferences() {
    let f = Fixture::new();
    let (uid, token) = user(&f.app, "A").await;
    let (other, _) = user(&f.app, "B").await;
    for (user, id, status, text) in [
        (&uid, "p", "active", "仅减少这个具体问题的重复研究建议"),
        (&uid, "off", "dismissed", "停用内容"),
        (&uid, "pending", "pending", "待确认内容"),
        (&other, "private", "active", "另一个用户的隐私"),
    ] {
        f.app.store.put(user,"cognition",&json!({"id":id,"type":"preference","statement":text,
            "status":status,"revision":1,"createdAt":now(),"evidenceRefs":[{"quote":"旧原话隐私"}]}),None).unwrap();
    }
    assert_eq!(
        cognition::formation_context(&f.app, &uid).unwrap(),
        vec![json!({"type":"preference","statement":"仅减少这个具体问题的重复研究建议"})]
    );
    assert_eq!(
        request(
            &f.app,
            "PUT",
            "/api/v1/cognition/p",
            &token,
            Some(json!({"statement":"已改为继续研究该具体问题","revision":1}))
        )
        .await
        .0,
        200
    );
    let current = cognition::formation_context(&f.app, &uid).unwrap();
    assert_eq!(
        current,
        vec![json!({"type":"preference","statement":"已改为继续研究该具体问题"})]
    );
    assert!(!json!(current).to_string().contains("旧原话"));
    assert_eq!(
        request(
            &f.app,
            "PUT",
            "/api/v1/cognition/p",
            &token,
            Some(json!({"status":"dismissed","revision":2}))
        )
        .await
        .0,
        200
    );
    assert!(cognition::formation_context(&f.app, &uid)
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn formation_context_is_bounded_without_cutting_off_scope_qualifiers() {
    let f = Fixture::new();
    let (uid, _) = user(&f.app, "A").await;
    for n in 0..40 {
        let text = format!(
            "{}仅限这一具体问题，不能扩展至整个领域。",
            "字".repeat(1950)
        );
        f.app
            .store
            .put(
                &uid,
                "cognition",
                &json!({"id":format!("p-{n:02}"),"type":"preference",
            "statement":text,"status":"active","createdAt":now()}),
                None,
            )
            .unwrap();
    }
    let bounded = cognition::formation_context(&f.app, &uid).unwrap();
    assert!(bounded.len() <= 32);
    assert!(
        bounded
            .iter()
            .map(|v| v["statement"].as_str().unwrap().chars().count())
            .sum::<usize>()
            <= 12_000
    );
    assert!(bounded.iter().all(|v| v["statement"]
        .as_str()
        .unwrap()
        .ends_with("不能扩展至整个领域。")));
    for n in 0..40 {
        f.app
            .store
            .put(
                &uid,
                "cognition",
                &json!({"id":format!("p-{n:02}"),"type":"interest",
            "statement":format!("问题{n}"),"status":"active","createdAt":now()}),
                None,
            )
            .unwrap();
    }
    assert_eq!(
        cognition::formation_context(&f.app, &uid).unwrap().len(),
        32
    );
}

#[tokio::test]
async fn changed_preferences_do_not_rerun_completed_paid_formation_windows() {
    let mut f = Fixture::new();
    let (uid, _) = user(&f.app, "A").await;
    f.app.hosted = std::sync::Arc::new(
        json!({"text":{"protocol":"openai","baseUrl":"https://example.com/v1","model":"fixture","apiKey":"fixture"}}),
    );
    let text = "已分析过的原始事实和原话";
    f.app.store.put(&uid,"source",&json!({"id":"s","text":text,"kind":"import","speakerMode":"self","status":"ready","createdAt":now()}),None).unwrap();
    let version = hash(format!("topics-v3:{text}").as_bytes());
    let end = text.chars().count();
    let key = format!("window-{}", hash(format!("s:{version}:0:{end}").as_bytes()));
    let completed = json!({"id":key,"sourceId":"s","sourceHash":version,"start":0,"end":end,"status":"ready","createdAt":now()});
    f.app
        .store
        .put(&uid, "formation_window", &completed, None)
        .unwrap();
    f.app.store.put(&uid,"cognition",&json!({"id":"p","type":"preference","statement":"少推荐这个具体问题的重复建议","status":"active","createdAt":now()}),None).unwrap();
    f.app
        .store
        .queue(&uid, "formation", json!({"sourceId":"s"}), "test")
        .unwrap();
    let job = f.app.store.claim().unwrap().unwrap();
    let output = formation::run(&f.app, &job).await.unwrap();
    assert_eq!(
        output.last().unwrap().1["analysisCoverage"]["complete"],
        true
    );
    assert_eq!(f.app.store.get(&uid, "source", "s").unwrap()["text"], text);
    assert_eq!(
        f.app
            .store
            .get(&uid, "formation_window", completed["id"].as_str().unwrap())
            .unwrap(),
        completed
    );
    let usage: i64 = f
        .app
        .store
        .conn()
        .unwrap()
        .query_row("SELECT count(*) FROM billing_usage", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        usage, 0,
        "adding preferences must not invalidate completed extraction or call the model again"
    );
}
