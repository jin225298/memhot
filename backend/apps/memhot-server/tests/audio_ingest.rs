mod common;
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use common::*;
use http_body_util::BodyExt;
use memhot_server::{api, audio, hash, ledger, now, sources, worker, App};
use serde_json::{json, Value};
use tower::ServiceExt;

fn clip() -> Vec<u8> {
    audio::wav(
        &vec![16; 32000],
        asr_service::AudioFormat {
            sample_rate: 16000,
            channels: 1,
            bits_per_sample: 16,
        },
    )
}
async fn upload(app: &App, token: &str, client: &str, data: &[u8]) -> (StatusCode, Value) {
    let req = Request::post("/api/v1/audio")
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "audio/wav")
        .header("idempotency-key", client)
        .body(Body::from(data.to_vec()))
        .unwrap();
    let response = api::router(app.clone()).oneshot(req).await.unwrap();
    (
        response.status(),
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap(),
    )
}
async fn configure(app: &App, token: &str) {
    assert_eq!(request(app,"PUT","/api/v1/profile",token,Some(json!({"models":{"asr":{"protocol":"openai","baseUrl":"https://example.com/v1","model":"fixture","apiKey":"fixture-only"}}}))).await.0, 200);
}
fn no_files(app: &App) {
    assert_eq!(
        std::fs::read_dir(app.data_dir.join("audio"))
            .map(|d| d.count())
            .unwrap_or(0),
        0
    );
}
#[tokio::test]
async fn completed_receipt_recovers_lost_response_once_and_empty_never_forms_topics() {
    for text in ["", "合成转写测试"] {
        let f = Fixture::new();
        let app = &f.app;
        let (uid, token) = user(app, "fixture").await;
        configure(app, &token).await;
        let data = clip();
        let client = "lost-response";
        let source_id = hash(format!("{uid}:{client}").as_bytes());
        let key = format!("audio:{source_id}");
        ledger::reserve_fingerprinted(app, &uid, None, &key, &hash(&data), "asr", 1).unwrap();
        ledger::settle_request(app, &uid, &key, &Ok(text.to_owned())).unwrap();
        for _ in 0..2 {
            let (status, result) = upload(app, &token, client, &data).await;
            assert_eq!(status, 202, "{result}");
            assert_eq!(
                result["status"],
                if text.is_empty() {
                    "no_speech"
                } else {
                    "ready"
                }
            );
        }
        let conn = app.store.conn().unwrap();
        let jobs: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM jobs WHERE kind='formation'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(jobs, if text.is_empty() { 0 } else { 1 });
        let receipt: Option<String> = conn
            .query_row("SELECT response FROM receipts WHERE key=?", [&key], |r| {
                r.get(0)
            })
            .unwrap();
        assert!(
            receipt.is_none(),
            "transcript copy is erased after source commits"
        );
        no_files(app);
    }
}
#[tokio::test]
async fn unknown_receipt_blocks_automatic_paid_replay_without_storing_audio() {
    let f = Fixture::new();
    let app = &f.app;
    let (uid, token) = user(app, "fixture").await;
    configure(app, &token).await;
    let data = clip();
    let client = "unknown";
    let source = hash(format!("{uid}:{client}").as_bytes());
    ledger::reserve_fingerprinted(
        app,
        &uid,
        None,
        &format!("audio:{source}"),
        &hash(&data),
        "asr",
        1,
    )
    .unwrap();
    for _ in 0..2 {
        let (_, result) = upload(app, &token, client, &data).await;
        assert_eq!(result["error"]["code"], "outcome_unknown");
    }
    assert_eq!(app.store.list(&uid, "source").unwrap().len(), 1);
    assert!(app.store.jobs(&uid).unwrap().is_empty());
    no_files(app);
}
#[tokio::test]
async fn local_text_is_idempotent_tenant_scoped_and_never_self_attributed() {
    let f = Fixture::new();
    let app = &f.app;
    let (a, ta) = user(app, "A").await;
    let (b, tb) = user(app, "B").await;
    let input = json!({"clientId":"same-device-clip","text":"合成会议片段","capturedAt":now(),"durationMs":1000,"modelId":"sensevoice-small-int8","modelVersion":"2024-07-17","audioHash":"a".repeat(64)});
    let (_, first) = request(
        app,
        "POST",
        "/api/v1/audio/transcript",
        &ta,
        Some(input.clone()),
    )
    .await;
    let (_, again) = request(
        app,
        "POST",
        "/api/v1/audio/transcript",
        &ta,
        Some(input.clone()),
    )
    .await;
    assert_eq!(first["status"], "ready");
    assert_eq!(first, again);
    let (_, other) = request(
        app,
        "POST",
        "/api/v1/audio/transcript",
        &tb,
        Some(input.clone()),
    )
    .await;
    assert_ne!(first["id"], other["id"]);
    for uid in [&a, &b] {
        let sources = app.store.list(uid, "source").unwrap();
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0]["speakerMode"], "unknown");
        assert_eq!(sources[0]["kind"], "audio");
        assert!(app.store.list(uid, "thought").unwrap().is_empty());
    }
    let mut changed = input;
    changed["text"] = json!("different");
    assert_eq!(
        request(app, "POST", "/api/v1/audio/transcript", &ta, Some(changed))
            .await
            .0,
        409
    );
    let receipts: i64 = app
        .store
        .conn()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM receipts", [], |r| r.get(0))
        .unwrap();
    assert_eq!(receipts, 0);
    no_files(app);
}
#[tokio::test]
async fn legacy_file_is_removed_even_when_model_configuration_is_missing() {
    let f = Fixture::new();
    let app = &f.app;
    let (uid, _) = user(app, "fixture").await;
    let data = clip();
    std::fs::create_dir_all(app.data_dir.join("audio")).unwrap();
    let path = app.data_dir.join("audio").join("legacy.wav");
    std::fs::write(&path, &data).unwrap();
    let source = json!({"id":"legacy","kind":"audio","status":"queued","createdAt":now(),"_hash":hash(&data),"_audioPath":"legacy.wav"});
    sources::persist_input(
        app,
        &uid,
        &source,
        None,
        Some("legacy-client"),
        "transcribe",
    )
    .unwrap();
    let job = app.store.claim().unwrap().unwrap();
    assert_eq!(
        worker::execute(app, &job).await.unwrap_err().code,
        "model_unconfigured"
    );
    assert!(!path.exists());
    assert_eq!(
        app.store.get(&uid, "source", "legacy").unwrap()["audioRecovery"],
        "reupload_same_segment"
    );
}
