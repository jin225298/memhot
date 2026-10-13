mod common;
use common::*;
use memhot_server::{audio, now, sources, App};
use serde_json::{json, Value};
// Simulate an older installed server's durable audio; the current upload route never creates it.
async fn failed_audio(app: &App, uid: &str, _token: &str) -> (String, Value) {
    let data = audio::wav(
        &vec![0; 320],
        asr_service::AudioFormat {
            sample_rate: 16000,
            channels: 1,
            bits_per_sample: 16,
        },
    );
    let id = memhot_server::id();
    let file = format!("{id}.wav");
    std::fs::create_dir_all(app.data_dir.join("audio")).unwrap();
    std::fs::write(app.data_dir.join("audio").join(&file), &data).unwrap();
    let source = json!({"id":id,"createdAt":now(),"kind":"audio","status":"queued","_audioPath":file,"_hash":memhot_server::hash(&data)});
    sources::persist_input(app, uid, &source, None, Some("retry-clip"), "transcribe").unwrap();
    let job = app.store.claim().unwrap().unwrap();
    app.store
        .conn()
        .unwrap()
        .execute("UPDATE jobs SET status='failed' WHERE id=?", [&job.id])
        .unwrap();
    (job.id, source)
}
#[tokio::test]
async fn uncertain_paid_retry_requires_explicit_confirmation_is_tenant_scoped_and_idempotent() {
    let f = Fixture::new();
    let app = &f.app;
    let (uid, token) = user(app, "A").await;
    let (_, other) = user(app, "B").await;
    let (job, _) = failed_audio(app, &uid, &token).await;
    app.store.conn().unwrap().execute("INSERT INTO receipts(user_id,job_id,key,status,created) VALUES(?,?,'uncertain','pending',?)",rusqlite::params![uid,job,chrono::Utc::now().timestamp()]).unwrap();
    let path = format!("/api/v1/jobs/{job}/retry");
    assert_eq!(
        request(
            app,
            "POST",
            &path,
            &other,
            Some(json!({"acceptDuplicateCharge":true}))
        )
        .await
        .0,
        404
    );
    let rejected = request(
        app,
        "POST",
        &path,
        &token,
        Some(json!({"acceptDuplicateCharge":false})),
    )
    .await;
    assert_eq!(rejected.0, 409);
    assert_eq!(
        rejected.1["error"]["code"],
        "duplication_confirmation_required"
    );
    let jobs = app.store.jobs(&uid).unwrap();
    assert_eq!(jobs[0]["requiresChargeConfirmation"], true);
    assert_eq!(jobs[0]["retryable"], true);
    let accepted = request(
        app,
        "POST",
        &path,
        &token,
        Some(json!({"acceptDuplicateCharge":true})),
    )
    .await;
    assert_eq!(accepted.0, 202);
    assert_ne!(accepted.1["id"], job);
    let duplicate = request(
        app,
        "POST",
        &path,
        &token,
        Some(json!({"acceptDuplicateCharge":false})),
    )
    .await;
    assert_eq!(accepted.1["id"], duplicate.1["id"]);
    let jobs = app.store.jobs(&uid).unwrap();
    assert_eq!(jobs.len(), 2);
    assert_eq!(
        jobs.iter().find(|j| j["id"] == job).unwrap()["retryable"],
        false
    );
    request(app,"PUT","/api/v1/profile",&token,Some(json!({"models":{"asr":{"protocol":"openai","baseUrl":"https://example.com/v1","model":"whisper","apiKey":"test"}}}))).await;
    let status: String = app
        .store
        .conn()
        .unwrap()
        .query_row("SELECT status FROM jobs WHERE id=?", [job], |r| r.get(0))
        .unwrap();
    assert_eq!(
        status, "failed",
        "config changes cannot silently requeue the replaced original"
    );
}
#[tokio::test]
async fn ordinary_retry_reuses_server_audio_and_rejects_missing_context() {
    let f = Fixture::new();
    let app = &f.app;
    let (uid, token) = user(app, "A").await;
    let (job, source) = failed_audio(app, &uid, &token).await;
    let path = format!("/api/v1/jobs/{job}/retry");
    std::fs::remove_file(
        app.data_dir
            .join("audio")
            .join(source["_audioPath"].as_str().unwrap()),
    )
    .unwrap();
    let response = request(
        app,
        "POST",
        &path,
        &token,
        Some(json!({"acceptDuplicateCharge":false})),
    )
    .await;
    assert_eq!(response.0, 409);
    assert_eq!(response.1["error"]["code"], "audio_expired");
    let src = note(app, &token, "note-retry").await;
    let job = app.store.claim().unwrap().unwrap();
    app.store
        .conn()
        .unwrap()
        .execute("UPDATE jobs SET status='failed' WHERE id=?", [&job.id])
        .unwrap();
    let retry = request(
        app,
        "POST",
        &format!("/api/v1/jobs/{}/retry", job.id),
        &token,
        Some(json!({"acceptDuplicateCharge":false})),
    )
    .await;
    assert_eq!(retry.0, 202);
    let queued = app.store.claim().unwrap().unwrap();
    assert_eq!(queued.payload["sourceId"], src["id"]);
    sources::delete_owned(app, &uid, src["id"].as_str().unwrap()).unwrap();
    assert!(!app.store.finish(&queued, &[], "too late").unwrap());
}
#[tokio::test]
async fn retry_is_bounded_and_nonquoted_prompt_dependencies_are_revoked() {
    let f = Fixture::new();
    let app = &f.app;
    let (uid, token) = user(app, "A").await;
    let first = note(app, &token, "source-A").await;
    let first_job = app.store.claim().unwrap().unwrap();
    app.store.finish(&first_job, &[], "done").unwrap();
    let mut thought = app.store.list(&uid, "thought").unwrap()[0].clone();
    let thought_id = thought["id"].as_str().unwrap().to_owned();
    let second = note(app, &token, "source-B").await;
    let job = app.store.claim().unwrap().unwrap();
    thought["sourceIds"] = json!([first["id"], second["id"]]);
    thought["title"] = json!("吸收了B但逐字引文来自A");
    thought["_mustExist"] = json!(true);
    assert!(app
        .store
        .finish(&job, &[("thought", thought)], "formed")
        .unwrap());
    app.store.put(&uid,"report",&json!({"id":"derived","createdAt":now(),"thoughtId":thought_id,"title":"private-B","body":"private-B","summary":"private-B","sources":[{"url":"https://example.com","excerpt":"public"}]}),None).unwrap();
    app.store.put(&uid,"conversation",&json!({"id":"discussion","thoughtId":thought_id,"reportId":"derived","messages":[{"text":"private-B"}],"createdAt":now()}),None).unwrap();
    sources::delete_owned(app, &uid, second["id"].as_str().unwrap()).unwrap();
    let remaining = app.store.get(&uid, "thought", &thought_id).unwrap();
    assert_eq!(remaining["sourceIds"], json!([first["id"]]));
    assert_eq!(remaining["analysisStatus"], "pending");
    assert!(!remaining.to_string().contains("吸收了B"));
    assert!(app.store.list(&uid, "conversation").unwrap().is_empty());
    let report = app.store.get(&uid, "report", "derived").unwrap();
    assert!(!report.to_string().contains("private-B"));
    assert_eq!(report["personalContextDeleted"], true);
    let ordinary = app
        .store
        .queue(&uid, "research", json!({"kind":"research"}), "ordinary")
        .unwrap();
    app.store
        .conn()
        .unwrap()
        .execute(
            "UPDATE jobs SET status='failed' WHERE id=?",
            [ordinary["id"].as_str().unwrap()],
        )
        .unwrap();
    for n in 0..8 {
        app.store
            .queue(
                &uid,
                "digest",
                json!({"kind":"digest"}),
                &format!("bound-{n}"),
            )
            .unwrap();
    }
    let response = request(
        app,
        "POST",
        &format!("/api/v1/jobs/{}/retry", ordinary["id"].as_str().unwrap()),
        &token,
        Some(json!({"acceptDuplicateCharge":false})),
    )
    .await;
    assert_eq!(response.0, 429);
    assert_eq!(response.1["error"]["code"], "retry_limit");
}
