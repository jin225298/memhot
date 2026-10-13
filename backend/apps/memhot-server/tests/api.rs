mod common;
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use common::*;
use http_body_util::BodyExt;
use memhot_server::{api, audio, now, worker, App};
use serde_json::{json, Value};
use tower::ServiceExt;
#[tokio::test]
async fn pairing_tenant_boundaries_and_revocation() {
    let f = Fixture::new();
    let app = &f.app;
    assert_eq!(
        request(app, "GET", "/api/v1/home", "", None).await.0,
        StatusCode::UNAUTHORIZED
    );
    let (a, ta) = user(app, "A").await;
    let (b, tb) = user(app, "B").await;
    let sa = note(app, &ta, "same-client").await;
    let sb = note(app, &tb, "same-client").await;
    assert_ne!(sa["id"], sb["id"]);
    let thought = app.store.list(&a, "thought").unwrap()[0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    for (method, path, input) in [
        (
            "GET",
            format!("/api/v1/sources/{}", sa["id"].as_str().unwrap()),
            None,
        ),
        (
            "DELETE",
            format!("/api/v1/sources/{}", sa["id"].as_str().unwrap()),
            None,
        ),
        (
            "POST",
            format!("/api/v1/thoughts/{thought}/feedback"),
            Some(json!({"action":"keep"})),
        ),
        (
            "POST",
            "/api/v1/research".into(),
            Some(json!({"thoughtId":thought})),
        ),
        (
            "POST",
            "/api/v1/discuss".into(),
            Some(json!({"thoughtId":thought,"message":"继续"})),
        ),
    ] {
        assert_eq!(
            request(app, method, &path, &tb, input).await.0,
            StatusCode::NOT_FOUND,
            "{}",
            path
        );
    }
    app.store
        .put(
            &a,
            "report",
            &json!({"id":"r-a","createdAt":now(),"body":"large private body","sources":[]}),
            None,
        )
        .unwrap();
    app.store.put(&a,"conversation",&json!({"id":"c-a","createdAt":now(),"thoughtId":thought,"messages":[{"text":"private"}]}),None).unwrap();
    assert_eq!(
        request(app, "GET", "/api/v1/reports/r-a", &tb, None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        request(app, "GET", "/api/v1/conversations/c-a", &tb, None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        request(app, "GET", "/api/v1/delivery", &tb, None).await.1["reports"],
        json!([])
    );
    let home = request(app, "GET", "/api/v1/home", &ta, None).await.1;
    assert!(home["sources"][0].get("text").is_none());
    assert_eq!(home["reports"][0]["body"], "");
    assert!(home["conversations"][0].get("messages").is_none());
    let raw: String = app
        .store
        .conn()
        .unwrap()
        .query_row(
            "SELECT token_hash FROM devices WHERE user_id=?",
            [&a],
            |r| r.get(0),
        )
        .unwrap();
    assert_ne!(raw, ta);
    assert_eq!(
        request(app, "DELETE", "/api/v1/device", &ta, None).await.0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        request(app, "GET", "/api/v1/home", &ta, None).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(app, "GET", "/api/v1/home", &tb, None).await.0,
        StatusCode::OK
    );
    assert!(!app.store.list(&b, "source").unwrap().is_empty());
}
#[tokio::test]
async fn inputs_are_idempotent_and_silence_keeps_no_cloud_audio() {
    let f = Fixture::new();
    let app = &f.app;
    let (uid, token) = user(app, "A").await;
    let first = note(app, &token, "note-1").await;
    assert_eq!(first["id"], note(app, &token, "note-1").await["id"]);
    assert_eq!(
        request(
            app,
            "POST",
            "/api/v1/sources",
            &token,
            Some(json!({"text":"different","clientId":"note-1"}))
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let data = audio::wav(
        &vec![0; 320],
        asr_service::AudioFormat {
            sample_rate: 16000,
            channels: 1,
            bits_per_sample: 16,
        },
    );
    async fn upload(app: &App, token: &str, data: Vec<u8>) -> (StatusCode, Value) {
        let req = Request::post("/api/v1/audio")
            .header("authorization", format!("Bearer {token}"))
            .header("content-type", "audio/wav")
            .header("idempotency-key", "segment-1")
            .body(Body::from(data))
            .unwrap();
        let response = api::router(app.clone()).oneshot(req).await.unwrap();
        let status = response.status();
        (
            status,
            serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes())
                .unwrap(),
        )
    }
    let (one, two) = tokio::join!(
        upload(app, &token, data.clone()),
        upload(app, &token, data.clone())
    );
    assert_eq!(one.0, StatusCode::ACCEPTED);
    assert_eq!(one.1["id"], two.1["id"]);
    assert_eq!(
        std::fs::read_dir(app.data_dir.join("audio"))
            .map(|d| d.count())
            .unwrap_or(0),
        0
    );
    assert_eq!(one.1["status"], "no_speech");
    assert_eq!(one.1["audioDeleted"], true);
    assert_eq!(
        app.store.list(&uid, "thought").unwrap().len(),
        1,
        "audio upload must not invent a thought"
    );
    let key = one.1["id"].as_str().unwrap();
    let source = app.store.get(&uid, "source", key).unwrap();
    assert!(source["_audioPath"].is_null());
    assert_eq!(source["analysisStatus"], "skipped");
    let receipts: i64 = app
        .store
        .conn()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM receipts", [], |r| r.get(0))
        .unwrap();
    assert_eq!(receipts, 0, "silence cannot invoke paid ASR");
    let mut different = data;
    different[44] = 1;
    assert_eq!(upload(app, &token, different).await.0, StatusCode::CONFLICT);
    assert_eq!(
        request(
            app,
            "POST",
            "/api/v1/discuss",
            &token,
            Some(json!({"message":"继续想"}))
        )
        .await
        .1["error"]["code"],
        "model_unconfigured"
    );
}
#[tokio::test]
async fn import_roles_and_configuration_recovery_are_explicit() {
    let f = Fixture::new();
    let app = &f.app;
    let (uid, token) = user(app, "A").await;
    let unknown = request(
        app,
        "POST",
        "/api/v1/sources",
        &token,
        Some(json!({"kind":"import","text":"没有角色的聊天记录"})),
    )
    .await
    .1;
    assert_eq!(unknown["analysisStatus"], "roles_required");
    assert!(app.store.list(&uid, "thought").unwrap().is_empty());
    let original = r#"{"messages":[{"role":"assistant","content":"你一定喜欢AI"},{"role":"user","content":"我还不确定该研究什么"}]}"#;
    let imported = request(
        app,
        "POST",
        "/api/v1/sources",
        &token,
        Some(json!({"kind":"import","text":original})),
    )
    .await
    .1;
    assert_eq!(imported["analysisStatus"], "pending");
    assert_eq!(imported["text"], original);
    while let Some(job) = app.store.claim().unwrap() {
        worker::execute(app, &job).await.unwrap();
    }
    assert!(app.store.list(&uid, "thought").unwrap().is_empty());
    let source = app
        .store
        .get(&uid, "source", imported["id"].as_str().unwrap())
        .unwrap();
    assert_eq!(source["analysisStatus"], "model_unconfigured");
    let settings = json!({"models":{"text":{"protocol":"openai","baseUrl":"https://example.com/v1","model":"test","apiKey":"secret"}}});
    assert_eq!(
        request(app, "PUT", "/api/v1/profile", &token, Some(settings))
            .await
            .0,
        StatusCode::OK
    );
    assert!(app
        .store
        .jobs(&uid)
        .unwrap()
        .iter()
        .any(|j| j["kind"] == "formation" && j["status"] == "queued"));
}
#[tokio::test]
async fn changing_provider_never_forwards_an_omitted_old_secret() {
    let f = Fixture::new();
    let app = &f.app;
    let (uid, token) = user(app, "A").await;
    let base = json!({"models":{"text":{"protocol":"openai","baseUrl":"https://example.com/v1","model":"one","apiKey":"secret-A"}},"search":{"provider":"tavily","apiKey":"secret-search"}});
    let response = request(app, "PUT", "/api/v1/profile", &token, Some(base))
        .await
        .1;
    assert_eq!(response["models"]["text"]["configured"], true);
    assert!(!response.to_string().contains("secret"));
    request(
        app,
        "PUT",
        "/api/v1/profile",
        &token,
        Some(json!({"models":{"text":{"model":"two"}}})),
    )
    .await;
    assert_eq!(
        app.store.profile(&uid).unwrap()["models"]["text"]["apiKey"],
        "secret-A"
    );
    request(
        app,
        "PUT",
        "/api/v1/profile",
        &token,
        Some(json!({"models":{"text":{"baseUrl":"https://other.example/v1"}}})),
    )
    .await;
    assert!(app.store.profile(&uid).unwrap()["models"]["text"]["apiKey"].is_null());
    request(
        app,
        "PUT",
        "/api/v1/profile",
        &token,
        Some(json!({"models":{"text":{"apiKey":"secret-B"}}})),
    )
    .await;
    request(
        app,
        "PUT",
        "/api/v1/profile",
        &token,
        Some(json!({"models":{"text":{"protocol":"anthropic"}}})),
    )
    .await;
    assert!(app.store.profile(&uid).unwrap()["models"]["text"]["apiKey"].is_null());
    assert_eq!(
        request(
            app,
            "PUT",
            "/api/v1/profile",
            &token,
            Some(json!({"models":{"text":{"baseUrl":"https://127.0.0.1/v1","apiKey":"secret"}}}))
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
}
#[tokio::test]
async fn android_cors_preflight_allows_upload_headers() {
    let f = Fixture::new();
    let req = Request::builder()
        .method("OPTIONS")
        .uri("/api/v1/audio")
        .header("origin", "https://tauri.localhost")
        .header("access-control-request-method", "POST")
        .header(
            "access-control-request-headers",
            "authorization,content-type,idempotency-key,x-captured-at",
        )
        .body(Body::empty())
        .unwrap();
    let response = api::router(f.app.clone()).oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()["access-control-allow-origin"],
        "https://tauri.localhost"
    );
}

#[tokio::test]
async fn unknown_speaker_import_preserves_original_without_a_user_attributed_seed() {
    let f = Fixture::new();
    let (uid, token) = user(&f.app, "Meeting owner").await;
    let (status, source) = request(&f.app, "POST", "/api/v1/sources", &token, Some(json!({
        "kind":"import", "speakerMode":"unknown", "text":"Speaker A: Is this supported?\nSpeaker B: I do not know."
    }))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(source["speakerMode"], "unknown");
    assert_eq!(source["analysisStatus"], "pending");
    assert!(f.app.store.list(&uid, "thought").unwrap().is_empty());
}
