mod common;
use axum::{
    body::{Body, Bytes},
    http::{Request, StatusCode},
};
use common::*;
use futures_util::stream;
use memhot_server::{api, audio, hash, ledger, App};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    task::Poll,
    time::Duration,
};
use tower::ServiceExt;

fn wav() -> Vec<u8> {
    audio::wav(
        &vec![16; 32000],
        asr_service::AudioFormat {
            sample_rate: 16000,
            channels: 1,
            bits_per_sample: 16,
        },
    )
}
fn request_body(token: &str, id: &str, body: Body) -> Request<Body> {
    Request::post("/api/v1/audio")
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "audio/wav")
        .header("idempotency-key", id)
        .body(body)
        .unwrap()
}
async fn hold_body(app: &App, token: &str) -> tokio::task::JoinHandle<()> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let mut tx = Some(tx);
    let body = Body::from_stream(stream::poll_fn(move |_| {
        if let Some(tx) = tx.take() {
            let _ = tx.send(());
        }
        Poll::Pending::<Option<Result<Bytes, std::io::Error>>>
    }));
    let router = api::router(app.clone());
    let request = request_body(token, "held", body);
    let task = tokio::spawn(async move {
        let _ = router.oneshot(request).await;
    });
    tokio::time::timeout(Duration::from_secs(2), rx)
        .await
        .unwrap()
        .unwrap();
    task
}
async fn assert_unread(app: &App, token: &str, status: StatusCode) {
    let read = Arc::new(AtomicBool::new(false));
    let observed = read.clone();
    let body = Body::from_stream(stream::once(async move {
        observed.store(true, Ordering::SeqCst);
        Ok::<_, std::io::Error>(Bytes::from(wav()))
    }));
    let response = api::router(app.clone())
        .oneshot(request_body(token, "other", body))
        .await
        .unwrap();
    assert_eq!(response.status(), status);
    assert!(
        !read.load(Ordering::SeqCst),
        "rejected audio must not be read into memory"
    );
}

#[tokio::test]
async fn per_user_admission_rejects_before_reading_audio_and_releases_on_disconnect() {
    let f = Fixture::new();
    let (_, token) = user(&f.app, "audio").await;
    let held = hold_body(&f.app, &token).await;
    assert_unread(&f.app, &token, StatusCode::BAD_GATEWAY).await;
    held.abort();
    let _ = held.await;
    let silent = audio::wav(
        &vec![0; 320],
        asr_service::AudioFormat {
            sample_rate: 16000,
            channels: 1,
            bits_per_sample: 16,
        },
    );
    let response = api::router(f.app.clone())
        .oneshot(request_body(&token, "after-disconnect", Body::from(silent)))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
}

#[tokio::test]
async fn global_admission_bounds_audio_memory_across_accounts() {
    let f = Fixture::new();
    let mut held = Vec::new();
    for n in 0..3 {
        let (_, token) = user(&f.app, &format!("audio-{n}")).await;
        held.push(hold_body(&f.app, &token).await);
    }
    let (_, token) = user(&f.app, "fourth").await;
    assert_unread(&f.app, &token, StatusCode::BAD_GATEWAY).await;
    for task in held {
        task.abort();
        let _ = task.await;
    }
}

#[tokio::test]
async fn authentication_precedes_audio_admission_and_body_reading() {
    let f = Fixture::new();
    assert_unread(&f.app, "", StatusCode::UNAUTHORIZED).await;
    let _chat = f
        .app
        .concurrency
        .model(memhot_server::concurrency::Lane::Chat, Some("test"))
        .await
        .unwrap();
    assert_eq!(f.app.asr_gate.available_permits(), 3);
}

#[tokio::test]
async fn text_capacity_cannot_delay_recovering_a_completed_audio_receipt() {
    let f = Fixture::new();
    let (uid, token) = user(&f.app, "receipt").await;
    request(&f.app,"PUT","/api/v1/profile",&token,Some(serde_json::json!({"models":{"asr":{"protocol":"openai","baseUrl":"https://example.com/v1","model":"fixture","apiKey":"fixture-only"}}}))).await;
    let data = wav();
    let source = hash(format!("{uid}:recover").as_bytes());
    let key = format!("audio:{source}");
    ledger::reserve_fingerprinted(&f.app, &uid, None, &key, &hash(&data), "asr", 1).unwrap();
    ledger::settle_request(&f.app, &uid, &key, &Ok("合成转写结果".into())).unwrap();
    let mut _text_slots = Vec::new();
    for lane in memhot_server::concurrency::Lane::ALL {
        _text_slots.push(f.app.concurrency.model(lane, Some("test")).await.unwrap());
    }
    let response = tokio::time::timeout(
        Duration::from_millis(250),
        api::router(f.app.clone()).oneshot(request_body(&token, "recover", Body::from(data))),
    )
    .await
    .expect("ASR replay must remain available while text slots are occupied")
    .unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    assert_eq!(
        f.app.store.list(&uid, "source").unwrap()[0]["text"],
        "合成转写结果"
    );
}
