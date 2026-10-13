mod common;
use common::*;
use memhot_server::App;
use serde_json::{json, Value};

async fn create(app: &App, token: &str, question: &str) -> Value {
    let (code, thought) = request(
        app,
        "POST",
        "/api/v1/thoughts",
        token,
        Some(json!({"question":question,"summary":"希望先找到支持与相反证据","category":"学习"})),
    )
    .await;
    assert_eq!(code, 200);
    thought
}
fn cognition(app: &App, user: &str, thought: &Value) -> Value {
    app.store
        .get(
            user,
            "cognition",
            &format!("thought-interest:{}", thought["id"].as_str().unwrap()),
        )
        .unwrap()
}

#[tokio::test]
async fn manual_questions_create_editable_interests_without_turning_questions_into_beliefs() {
    let f = Fixture::new();
    let (a, token) = user(&f.app, "A").await;
    let (b, other) = user(&f.app, "B").await;
    let thought = create(&f.app, &token, "咖啡是否一定有益健康？").await;
    let key = thought["id"].as_str().unwrap();
    let interest = cognition(&f.app, &a, &thought);
    assert_eq!(interest["type"], "interest");
    assert_eq!(interest["origin"], "explicit");
    assert_eq!(interest["status"], "active");
    assert!(interest["statement"]
        .as_str()
        .unwrap()
        .contains("主动研究的问题"));
    assert!(interest["inferenceReason"]
        .as_str()
        .unwrap()
        .contains("不代表观点或信念"));
    assert_eq!(interest["evidenceRefs"][0]["quote"], thought["question"]);
    assert_eq!(
        interest["evidenceRefs"][0]["explanation"],
        thought["summary"]
    );
    assert!(f.app.store.list_all(&b, "cognition").unwrap().is_empty());
    assert_eq!(
        request(
            &f.app,
            "PUT",
            &format!("/api/v1/thoughts/{key}"),
            &other,
            Some(json!({"question":"越权"}))
        )
        .await
        .0,
        404
    );
    let (_, updated) = request(
        &f.app,
        "PUT",
        &format!("/api/v1/thoughts/{key}"),
        &token,
        Some(json!({"question":"咖啡影响睡眠的证据是什么？","revision":1})),
    )
    .await;
    assert_eq!(updated["revision"], 2);
    let next = cognition(&f.app, &a, &thought);
    assert_eq!(next["id"], interest["id"]);
    assert_eq!(next["revision"], 2);
    assert!(next["statement"].as_str().unwrap().contains("睡眠"));
    assert_eq!(f.app.store.list_all(&a, "cognition").unwrap().len(), 1);
    let cog = next["id"].as_str().unwrap();
    let (_, owned) = request(
        &f.app,
        "PUT",
        &format!("/api/v1/cognition/{cog}"),
        &token,
        Some(json!({"statement":"只关注证据质量","status":"dismissed","revision":2})),
    )
    .await;
    assert_eq!(owned["revision"], 3);
    assert_eq!(
        request(
            &f.app,
            "PUT",
            &format!("/api/v1/thoughts/{key}"),
            &token,
            Some(json!({"question":"新的主动研究问题","revision":2}))
        )
        .await
        .0,
        200
    );
    assert_eq!(
        cognition(&f.app, &a, &thought),
        owned,
        "human-edited or disabled cognition must win over topic edits"
    );
}

#[tokio::test]
async fn archived_and_restored_thoughts_do_not_create_negative_memories_and_check_revision() {
    let f = Fixture::new();
    let (a, token) = user(&f.app, "A").await;
    let thought = create(&f.app, &token, "如何减少学习中的遗忘？").await;
    let key = thought["id"].as_str().unwrap();
    let interest = cognition(&f.app, &a, &thought);
    for (action, revision, status) in [
        ("dismiss", 1, "dismissed"),
        ("keep", 2, "kept"),
        ("resolved", 3, "resolved"),
    ] {
        let (code, value) = request(
            &f.app,
            "POST",
            &format!("/api/v1/thoughts/{key}/feedback"),
            &token,
            Some(json!({"action":action,"revision":revision})),
        )
        .await;
        assert_eq!(code, 200);
        assert_eq!(value["status"], status);
        assert_eq!(value["researchEligible"], action == "keep");
        assert_eq!(cognition(&f.app, &a, &thought), interest);
    }
    assert_eq!(f.app.store.list_all(&a, "cognition").unwrap().len(), 1);
    assert!(f.app.store.list_all(&a, "observation").unwrap().is_empty());
    assert_eq!(
        request(
            &f.app,
            "POST",
            &format!("/api/v1/thoughts/{key}/feedback"),
            &token,
            Some(json!({"action":"keep","revision":1}))
        )
        .await
        .0,
        409
    );
    let (_, archived) = request(
        &f.app,
        "GET",
        "/api/v1/thoughts?status=archived",
        &token,
        None,
    )
    .await;
    assert_eq!(archived["total"], 1);
    assert_eq!(
        request(
            &f.app,
            "PUT",
            &format!("/api/v1/thoughts/{key}"),
            &token,
            Some(json!({"status":"open","category":"研究","revision":4}))
        )
        .await
        .0,
        200
    );
    let (_, active) = request(
        &f.app,
        "GET",
        "/api/v1/thoughts?status=active",
        &token,
        None,
    )
    .await;
    assert_eq!(active["total"], 1);
    assert_eq!(active["items"][0]["category"], "研究");
}

#[tokio::test]
async fn delete_less_persists_one_explicit_scoped_preference_after_context_cleanup() {
    let f = Fixture::new();
    let (a, token) = user(&f.app, "A").await;
    let (b, other) = user(&f.app, "B").await;
    let thought = create(&f.app, &token, "这次是否需要购买这台相机？").await;
    let key = thought["id"].as_str().unwrap();
    f.app
        .store
        .put(
            &a,
            "source",
            &json!({"id":"source","text":"相机资料","createdAt":memhot_server::now()}),
            None,
        )
        .unwrap();
    assert_eq!(
        request(
            &f.app,
            "PUT",
            &format!("/api/v1/thoughts/{key}"),
            &token,
            Some(json!({"sourceIds":["source"],"revision":1}))
        )
        .await
        .0,
        200
    );
    assert_eq!(
        request(
            &f.app,
            "DELETE",
            &format!("/api/v1/thoughts/{key}?preference=less&revision=2"),
            &other,
            None
        )
        .await
        .0,
        404
    );
    assert_eq!(
        request(
            &f.app,
            "DELETE",
            &format!("/api/v1/thoughts/{key}?preference=less&revision=1"),
            &token,
            None
        )
        .await
        .0,
        409
    );
    assert_eq!(f.app.store.list_all(&a, "cognition").unwrap().len(), 1);
    assert_eq!(
        request(
            &f.app,
            "DELETE",
            &format!("/api/v1/thoughts/{key}?preference=less&revision=2"),
            &token,
            None
        )
        .await
        .0,
        204
    );
    assert!(f.app.store.get(&a, "thought", key).is_err());
    let memories = f.app.store.list_all(&a, "cognition").unwrap();
    assert_eq!(memories.len(), 1);
    let preference = &memories[0];
    assert_eq!(preference["type"], "preference");
    assert_eq!(preference["origin"], "explicit");
    assert_eq!(preference["scope"]["question"], thought["question"]);
    assert_eq!(preference["evidenceRefs"][0]["operation"], "delete_thought");
    assert_eq!(preference["evidenceRefs"][0]["preference"], "less");
    assert!(preference["statement"]
        .as_str()
        .unwrap()
        .contains("不代表对相关领域不感兴趣"));
    assert_eq!(
        request(
            &f.app,
            "DELETE",
            &format!("/api/v1/thoughts/{key}?preference=less&revision=2"),
            &token,
            None
        )
        .await
        .0,
        204
    );
    assert_eq!(
        f.app.store.list_all(&a, "cognition").unwrap(),
        memories,
        "retry cannot create a second memory or edit its revision"
    );
    memhot_server::sources::delete_owned(&f.app, &a, "source").unwrap();
    assert_eq!(
        f.app.store.list_all(&a, "cognition").unwrap(),
        memories,
        "independent operation preference survives source cleanup"
    );
    assert!(f.app.store.list_all(&b, "cognition").unwrap().is_empty());
    let pref = preference["id"].as_str().unwrap();
    assert_eq!(
        request(
            &f.app,
            "PUT",
            &format!("/api/v1/cognition/{pref}"),
            &token,
            Some(json!({"statement":"仅减少这款相机的购买建议","status":"dismissed","revision":1}))
        )
        .await
        .0,
        200
    );
    request(
        &f.app,
        "DELETE",
        &format!("/api/v1/thoughts/{key}?preference=less"),
        &token,
        None,
    )
    .await;
    assert_eq!(
        f.app.store.get(&a, "cognition", pref).unwrap()["statement"],
        "仅减少这款相机的购买建议"
    );
}

#[tokio::test]
async fn default_delete_adds_no_preference_and_deleted_interest_is_not_recreated() {
    let f = Fixture::new();
    let (a, token) = user(&f.app, "A").await;
    let thought = create(&f.app, &token, "如何整理阅读笔记？").await;
    let key = thought["id"].as_str().unwrap();
    let cog = cognition(&f.app, &a, &thought)["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        request(
            &f.app,
            "DELETE",
            &format!("/api/v1/cognition/{cog}"),
            &token,
            None
        )
        .await
        .0,
        204
    );
    assert_eq!(
        request(
            &f.app,
            "PUT",
            &format!("/api/v1/thoughts/{key}"),
            &token,
            Some(json!({"question":"更新后的问题"}))
        )
        .await
        .0,
        200
    );
    assert!(f.app.store.list_all(&a, "cognition").unwrap().is_empty());
    assert_eq!(
        request(
            &f.app,
            "DELETE",
            &format!("/api/v1/thoughts/{key}?preference=none"),
            &token,
            None
        )
        .await
        .0,
        204
    );
    assert!(f.app.store.list_all(&a, "cognition").unwrap().is_empty());
    assert_eq!(
        request(
            &f.app,
            "DELETE",
            &format!("/api/v1/thoughts/{key}"),
            &token,
            None
        )
        .await
        .0,
        204
    );
    assert_eq!(
        request(
            &f.app,
            "PUT",
            &format!("/api/v1/thoughts/{key}"),
            &token,
            Some(json!({"status":"open"}))
        )
        .await
        .0,
        404
    );
    assert_eq!(
        request(
            &f.app,
            "DELETE",
            "/api/v1/thoughts/unknown?preference=less",
            &token,
            None
        )
        .await
        .0,
        404
    );
    assert_eq!(
        request(
            &f.app,
            "DELETE",
            "/api/v1/thoughts/unknown?preference=all",
            &token,
            None
        )
        .await
        .0,
        400
    );
    assert_eq!(
        request(
            &f.app,
            "POST",
            "/api/v1/thoughts",
            &token,
            Some(json!({"question":"有效问题","category":3}))
        )
        .await
        .0,
        400
    );
    assert_eq!(
        request(
            &f.app,
            "POST",
            "/api/v1/thoughts",
            &token,
            Some(json!({"question":"有效问题","category":"字".repeat(41)}))
        )
        .await
        .0,
        400
    );
}
