use super::*;

fn material() -> Material {
    Material {
        title: "Limited preview".into(),
        url: "https://example.org/preview".into(),
        text: "The model is available only in a limited preview.".into(),
        published: Some("2026-10-12T00:00:00Z".into()),
        discovered: None,
        discovered_via: None,
        publication_source: "page",
        publisher: "Example".into(),
        fulltext: true,
        pdf: None,
    }
}
fn item(id: &str) -> Value {
    let evidence = material().evidence()["id"].clone();
    json!({"id":id,"eventKey":format!("event-{id}"),"title":"模型正式发布",
        "summary":"模型已经全面可用。","whyItMatters":"编辑推断：可评估是否采用。",
        "readingGuide":"核对预览的使用条件。","limitations":"仅有厂商自述。",
        "category":"产品","tags":["预览"],"reportingKind":"development",
        "facts":[{"index":0,"evidenceId":evidence,"quote":"available only in a limited preview"}],
        "evidenceIds":[evidence],"publishedAt":"2026-10-12T00:00:00+00:00","coverageCount":1})
}
fn verdict(id: &str, approved: bool) -> Value {
    json!({"id":id,"weakestStatement":"模型已经全面可用。","sourceLimit":"仅有限预览。",
        "contradiction":"","supported":approved,"relevant":true,
        "noUnsupportedDetail":approved,"reason":if approved {"所有断言均有来源。"} else {"不能把有限预览写成全面可用。"}})
}
fn edit(id: &str) -> Value {
    let original = item(id);
    json!({"id":id,"action":"patch","reason":"收窄可用范围至有限预览。","patch":{
        "title":"模型开放有限预览","summary":"模型目前仅开放有限预览。",
        "whyItMatters":original["whyItMatters"],"readingGuide":original["readingGuide"],
        "limitations":original["limitations"]}})
}
#[test]
fn patches_preserve_all_original_facts_evidence_dates_and_identity() {
    let original = item("a");
    let output = apply(&[original.clone()], &json!({"revisions":[edit("a")]})).unwrap();
    assert_eq!(output.len(), 1);
    assert_eq!(output[0]["summary"], "模型目前仅开放有限预览。");
    for (field, value) in original.as_object().unwrap() {
        if ![
            "title",
            "summary",
            "whyItMatters",
            "readingGuide",
            "limitations",
        ]
        .contains(&field.as_str())
        {
            assert_eq!(&output[0][field], value, "{field}");
        }
    }
    let output = apply(
        &[original],
        &json!({"revisions":[{"id":"a","action":"drop","reason":"原证据无法修复。"}]}),
    )
    .unwrap();
    assert!(output.is_empty());
}
#[test]
fn revisions_reject_unknown_duplicate_missing_ids_and_immutable_fields() {
    let original = item("a");
    for revisions in [
        json!([]),
        json!([edit("unknown")]),
        json!([edit("a"), edit("a")]),
        json!([{"id":"a","action":"keep","reason":"忽略审核。"}]),
        json!([{"id":"a","action":"drop","reason":"","patch":{}}]),
        json!([{"id":"a","action":"drop","reason":"无法修复。","patch":{}}]),
    ] {
        assert_eq!(
            apply(&[original.clone()], &json!({"revisions":revisions}))
                .unwrap_err()
                .code,
            "editorial_revision_invalid"
        );
    }
    for field in [
        "id",
        "eventKey",
        "facts",
        "evidenceIds",
        "publishedAt",
        "reportingKind",
        "category",
        "tags",
    ] {
        let mut row = edit("a");
        row["patch"][field] = json!("cannot-change");
        assert!(
            apply(&[original.clone()], &json!({"revisions":[row]})).is_err(),
            "patch {field}"
        );
        let mut row = edit("a");
        row[field] = json!("cannot-change");
        assert!(
            apply(&[original.clone()], &json!({"revisions":[row]})).is_err(),
            "edit {field}"
        );
    }
    assert!(apply(
        &[original.clone(), original],
        &json!({"revisions":[edit("a")]})
    )
    .is_err());
    assert!(apply(&[item("a")], &json!({"revisions":[edit("a")],"stories":[]})).is_err());
}
#[test]
fn revision_strings_are_bounded_without_clipping_or_null_and_noops_fail() {
    for (field, max) in [
        ("title", 80),
        ("summary", 2400),
        ("whyItMatters", 700),
        ("readingGuide", 500),
        ("limitations", 700),
    ] {
        let mut row = edit("a");
        row["patch"][field] = json!("中".repeat(max));
        assert!(
            apply(&[item("a")], &json!({"revisions":[row.clone()]})).is_ok(),
            "max {field}"
        );
        for bad in [
            json!("中".repeat(max + 1)),
            json!(" "),
            Value::Null,
            json!(123),
        ] {
            row["patch"][field] = bad;
            assert!(
                apply(&[item("a")], &json!({"revisions":[row.clone()]})).is_err(),
                "invalid {field}"
            );
        }
    }
    let original = item("a");
    let mut row = edit("a");
    for field in [
        "title",
        "summary",
        "whyItMatters",
        "readingGuide",
        "limitations",
    ] {
        row["patch"][field] = original[field].clone();
    }
    assert!(apply(&[original], &json!({"revisions":[row]})).is_err());
    let mut retrospective = item("a");
    retrospective["reportingKind"] = json!("retrospective");
    assert!(apply(&[retrospective], &json!({"revisions":[edit("a")]})).is_err());
}
#[test]
fn reviews_fail_closed_for_incomplete_duplicate_unknown_and_non_boolean_verdicts() {
    let items = vec![item("a"), item("b")];
    for rows in [
        json!([]),
        json!([verdict("a", true)]),
        json!([verdict("a", true), verdict("a", false)]),
        json!([verdict("a", true), verdict("unknown", true)]),
    ] {
        assert_eq!(
            checks(&items, &json!({"checks":rows})).err().unwrap().code,
            "editorial_review_invalid"
        );
    }
    for field in [
        "supported",
        "relevant",
        "noUnsupportedDetail",
        "weakestStatement",
        "sourceLimit",
        "contradiction",
        "reason",
    ] {
        let mut row = verdict("a", true);
        row.as_object_mut().unwrap().remove(field);
        assert!(
            checks(&[item("a")], &json!({"checks":[row]})).is_err(),
            "missing {field}"
        );
    }
    for field in ["supported", "relevant", "noUnsupportedDetail"] {
        let mut row = verdict("a", true);
        row[field] = json!("true");
        assert!(checks(&[item("a")], &json!({"checks":[row]})).is_err());
        let mut row = verdict("a", true);
        row[field] = json!(false);
        assert!(!checks(&[item("a")], &json!({"checks":[row]})).unwrap()["a"].approved());
    }
    let mut row = verdict("a", true);
    row["reason"] = json!("x".repeat(2401));
    assert!(checks(&[item("a")], &json!({"checks":[row]})).is_err());
}
#[test]
fn repair_and_recheck_identity_is_stable_and_recheck_has_no_prior_verdict() {
    let items = vec![item("a")];
    let review = checks(&items, &json!({"checks":[verdict("a", false)]})).unwrap();
    let prompt = revision_prompt("机器人", &items, &[material()], &review);
    let key = purpose("revise", REVISE, &prompt);
    assert!(key.starts_with("digest-fact-revise-v1-"));
    assert_eq!(
        key,
        purpose(
            "revise",
            REVISE,
            &revision_prompt("机器人", &items, &[material()], &review)
        )
    );
    let mut changed = prompt.clone();
    changed["stories"][0]["factReview"]["reason"] = json!("新的审核意见。");
    assert_ne!(key, purpose("revise", REVISE, &changed));
    let revised = apply(&items, &json!({"revisions":[edit("a")]})).unwrap();
    let recheck = recheck_prompt("机器人", &revised, &[material()]);
    assert!(
        purpose("recheck", REVIEW_INSTRUCTIONS, &recheck).starts_with("digest-fact-recheck-v1-")
    );
    assert!(recheck["stories"][0].get("factReview").is_none());
    assert!(recheck["stories"][0].get("patch").is_none());
    assert!(recheck.get("asOf").is_none());
    assert_eq!(recheck["stories"][0]["facts"], items[0]["facts"]);
    assert_eq!(
        recheck["stories"][0]["citedMaterials"][0]["text"],
        material().text
    );
}

fn completed(app: &App, scope: &Scope, purpose: &str, answer: &Value) {
    let saved = json!({"input":{"mode":"review","model":{"protocol":"openai","model":"test"}},
        "context":{"materials":[]},"step":1,"messages_json":"","pending_models":[],
        "pending_tools":[],"complete":answer.to_string(),"epoch":null,"created_at":crate::now()});
    app.store.conn().unwrap().execute_batch("CREATE TABLE IF NOT EXISTS public_agent_runs(run_key TEXT PRIMARY KEY,data TEXT NOT NULL,created INTEGER NOT NULL);").unwrap();
    app.store
        .conn()
        .unwrap()
        .execute(
            "INSERT INTO public_agent_runs(run_key,data,created) VALUES(?,?,?)",
            rusqlite::params![
                scope.key(purpose),
                saved.to_string(),
                chrono::Utc::now().timestamp()
            ],
        )
        .unwrap();
}
#[tokio::test]
async fn one_repair_and_independent_recheck_replay_without_calls_or_revising_approved() {
    let dir = std::env::temp_dir().join(format!("memhot-fact-revision-{}", crate::id()));
    let mut app = App::open(dir.clone()).unwrap();
    app.hosted = std::sync::Arc::new(
        json!({"text":{"protocol":"openai","model":"test","baseUrl":"https://example.invalid/v1","apiKey":"unused"}}),
    );
    let scope = Scope::public("bounded-revision-checkpoint", "ai").unwrap();
    let items = vec![
        item("approved"),
        item("repair"),
        item("drop"),
        item("still-rejected"),
    ];
    let review = json!({"checks":[verdict("approved", true), verdict("repair", false), verdict("drop", false), verdict("still-rejected", false)]});
    let verdicts = checks(&items, &review).unwrap();
    let rejected = items[1..].to_vec();
    let prompt = revision_prompt("机器人", &rejected, &[material()], &verdicts);
    assert_eq!(prompt["stories"].as_array().unwrap().len(), 3);
    assert!(prompt["stories"]
        .as_array()
        .unwrap()
        .iter()
        .all(|s| s["id"] != "approved"));
    let edits = json!({"revisions":[edit("repair"),{"id":"drop","action":"drop","reason":"原证据不足。"},edit("still-rejected")]});
    completed(&app, &scope, &purpose("revise", REVISE, &prompt), &edits);
    let revised = apply(&rejected, &edits).unwrap();
    let recheck = recheck_prompt("机器人", &revised, &[material()]);
    assert_eq!(recheck["stories"].as_array().unwrap().len(), 2);
    completed(
        &app,
        &scope,
        &purpose("recheck", REVIEW_INSTRUCTIONS, &recheck),
        &json!({"checks":[verdict("repair", true),verdict("still-rejected", false)]}),
    );
    for _ in 0..2 {
        let output = repair(&app, &scope, "机器人", &items, &[material()], &review)
            .await
            .unwrap();
        assert_eq!(output.len(), 2);
        assert_eq!(output[0], items[0]);
        assert_eq!(output[1]["id"], "repair");
        assert_eq!(output[1]["summary"], "模型目前仅开放有限预览。");
    }
    let conn = app.store.conn().unwrap();
    let runs: i64 = conn
        .query_row("SELECT count(*) FROM public_agent_runs", [], |r| r.get(0))
        .unwrap();
    assert_eq!(runs, 2);
    let calls: i64 = conn
        .query_row("SELECT count(*) FROM public_model_calls", [], |r| r.get(0))
        .unwrap();
    assert_eq!(calls, 0);
    drop(conn);
    drop(app);
    std::fs::remove_dir_all(dir).unwrap();
}
#[tokio::test]
async fn already_approved_stories_skip_both_stages() {
    let dir = std::env::temp_dir().join(format!("memhot-no-revision-{}", crate::id()));
    let app = App::open(dir.clone()).unwrap();
    let scope = Scope::public("all-approved", "ai").unwrap();
    let items = vec![item("a")];
    assert_eq!(
        repair(
            &app,
            &scope,
            "机器人",
            &items,
            &[],
            &json!({"checks":[verdict("a", true)]})
        )
        .await
        .unwrap(),
        items
    );
    drop(app);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn semantic_failure_reuses_completed_revision_without_unknown_billing_state() {
    let dir = std::env::temp_dir().join(format!("memhot-invalid-revision-{}", crate::id()));
    let mut app = App::open(dir.clone()).unwrap();
    app.hosted = std::sync::Arc::new(
        json!({"text":{"protocol":"openai","model":"test","baseUrl":"https://example.invalid/v1","apiKey":"unused"}}),
    );
    let scope = Scope::public("invalid-revision", "ai").unwrap();
    let items = vec![item("a")];
    let review = json!({"checks":[verdict("a", false)]});
    let verdicts = checks(&items, &review).unwrap();
    let prompt = revision_prompt("机器人", &items, &[material()], &verdicts);
    completed(
        &app,
        &scope,
        &purpose("revise", REVISE, &prompt),
        &json!({"revisions":[edit("a"),edit("a")]}),
    );
    for _ in 0..2 {
        let error = repair(&app, &scope, "机器人", &items, &[material()], &review)
            .await
            .unwrap_err();
        assert_eq!(error.code, "editorial_revision_invalid");
    }
    let conn = app.store.conn().unwrap();
    let runs: i64 = conn
        .query_row("SELECT count(*) FROM public_agent_runs", [], |r| r.get(0))
        .unwrap();
    assert_eq!(runs, 1);
    let calls: i64 = conn
        .query_row("SELECT count(*) FROM public_model_calls", [], |r| r.get(0))
        .unwrap();
    assert_eq!(calls, 0);
    drop(conn);
    drop(app);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn dropping_all_rejected_stories_skips_recheck_and_cannot_resurrect_them() {
    let dir = std::env::temp_dir().join(format!("memhot-drop-revision-{}", crate::id()));
    let mut app = App::open(dir.clone()).unwrap();
    app.hosted = std::sync::Arc::new(
        json!({"text":{"protocol":"openai","model":"test","baseUrl":"https://example.invalid/v1","apiKey":"unused"}}),
    );
    let scope = Scope::public("drop-revision", "ai").unwrap();
    let items = vec![item("a")];
    let review = json!({"checks":[verdict("a", false)]});
    let verdicts = checks(&items, &review).unwrap();
    let prompt = revision_prompt("机器人", &items, &[material()], &verdicts);
    completed(
        &app,
        &scope,
        &purpose("revise", REVISE, &prompt),
        &json!({"revisions":[{"id":"a","action":"drop","reason":"原证据不足以修复。"}]}),
    );
    assert!(
        repair(&app, &scope, "机器人", &items, &[material()], &review)
            .await
            .unwrap()
            .is_empty()
    );
    let conn = app.store.conn().unwrap();
    let runs: i64 = conn
        .query_row("SELECT count(*) FROM public_agent_runs", [], |r| r.get(0))
        .unwrap();
    assert_eq!(runs, 1);
    let calls: i64 = conn
        .query_row("SELECT count(*) FROM public_model_calls", [], |r| r.get(0))
        .unwrap();
    assert_eq!(calls, 0);
    drop(conn);
    drop(app);
    std::fs::remove_dir_all(dir).unwrap();
}
