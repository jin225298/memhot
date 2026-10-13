use crate::{
    audio,
    auth::{self, User},
    discussion,
    error::Result,
    profile, research, sources, App,
};
use axum::{
    extract::{DefaultBodyLimit, State},
    http::{HeaderValue, Method, StatusCode},
    response::IntoResponse,
    routing::{delete, get, post},
    Json, Router,
};
use serde_json::{json, Value};
use tower_http::cors::CorsLayer;

pub fn router(app: App) -> Router {
    let cors = CorsLayer::new()
        .allow_origin([
            HeaderValue::from_static("https://tauri.localhost"),
            HeaderValue::from_static("http://tauri.localhost"),
            HeaderValue::from_static("tauri://localhost"),
            HeaderValue::from_static("http://localhost:1420"),
        ])
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers([
            axum::http::header::AUTHORIZATION,
            axum::http::header::CONTENT_TYPE,
            axum::http::HeaderName::from_static("idempotency-key"),
            axum::http::HeaderName::from_static("x-captured-at"),
            axum::http::HeaderName::from_static("x-audio-retry-key"),
            axum::http::HeaderName::from_static("x-accept-duplicate-charge"),
        ]);
    Router::new()
        .route(
            "/healthz",
            get(|| async { Json(json!({"status":"ok","version":env!("CARGO_PKG_VERSION")})) }),
        )
        .route("/api/v1/pair", post(auth::pair))
        .route("/api/v1/auth/register", post(crate::accounts::register))
        .route("/api/v1/auth/login", post(crate::accounts::login))
        .route("/api/v1/auth/reset", post(crate::accounts::reset_password))
        .route("/api/v1/account", get(crate::accounts::account))
        .route("/api/v1/account/sessions", get(crate::accounts::sessions))
        .route(
            "/api/v1/account/sessions/{id}",
            delete(crate::accounts::revoke_session),
        )
        .route("/api/v1/billing/topups", post(crate::billing::topup))
        .route("/api/v1/billing/plan", post(crate::billing::plan))
        .route(
            "/api/v1/cognition",
            get(crate::cognition::list).post(crate::cognition::create),
        )
        .route(
            "/api/v1/cognition/{id}",
            axum::routing::put(crate::cognition::update).delete(crate::cognition::delete),
        )
        .route("/api/v1/daily-summaries", get(crate::daily::list))
        .route(
            "/api/v1/thoughts",
            get(crate::thought_list::list).post(crate::topics::create),
        )
        .route(
            "/api/v1/thoughts/{id}",
            axum::routing::put(crate::topics::update).delete(crate::topics::delete),
        )
        .route("/api/v1/device", delete(auth::revoke_device))
        .route("/api/v1/home", get(home))
        .route("/api/v1/delivery", get(delivery))
        .route("/api/v1/profile", get(profile::get).put(profile::update))
        .route("/api/v1/models/test", post(crate::models::test))
        .route("/api/v1/sources", post(sources::create))
        .route("/api/v1/sources/{id}/analyze", post(sources::analyze))
        .route(
            "/api/v1/sources/{id}",
            get(sources::get).delete(sources::delete),
        )
        .route("/api/v1/audio", post(audio::upload))
        .route("/api/v1/audio/transcript", post(audio::upload_transcript))
        .route("/api/v1/research", post(research::queue))
        .route("/api/v1/reading", get(crate::reading::list))
        .route(
            "/api/v1/reports/{id}/saved",
            axum::routing::put(crate::reading::set_saved),
        )
        .route(
            "/api/v1/reports/{id}/events/{storyId}",
            get(crate::reading::event),
        )
        .route(
            "/api/v1/reports/{id}/materials/{evidenceId}",
            get(crate::reading::material),
        )
        .route(
            "/api/v1/reports/{id}/materials/{evidenceId}/translation",
            post(crate::reading::queue_translation),
        )
        .route("/api/v1/jobs/{id}/retry", post(crate::retry::retry))
        .route(
            "/api/v1/reports/{id}",
            get(research::get).delete(research::delete),
        )
        .route("/api/v1/thoughts/{id}/feedback", post(sources::feedback))
        .route(
            "/api/v1/preferences/setup",
            get(crate::preferences_setup::get).post(crate::preferences_setup::submit),
        )
        .route("/api/v1/discuss", post(discussion::discuss))
        .route("/api/v1/conversations/{id}", get(discussion::get))
        .route("/api/v1/account/data", delete(clear))
        .fallback(|| async { crate::error::Error::missing().into_response() })
        .layer(DefaultBodyLimit::max(8 * 1024 * 1024))
        .layer(axum::middleware::from_fn(json_errors))
        .layer(cors)
        .with_state(app)
}
async fn json_errors(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let response = next.run(request).await;
    let status = response.status();
    if (status.is_client_error() || status.is_server_error())
        && response
            .headers()
            .get("content-type")
            .and_then(|h| h.to_str().ok())
            .is_none_or(|s| !s.starts_with("application/json"))
    {
        return crate::error::Error::new(
            status,
            if status == StatusCode::PAYLOAD_TOO_LARGE {
                "payload_too_large"
            } else {
                "invalid_input"
            },
            "请求格式无效或超出服务限制",
        )
        .into_response();
    }
    response
}
async fn delivery(State(app): State<App>, User(user): User) -> Result<Json<Value>> {
    Ok(Json(
        json!({"reports":app.store.list(&user,"report")?.iter().filter(|r|research::available(r)).map(|r|json!({"id":r["id"],"createdAt":r["createdAt"]})).collect::<Vec<_>>()}),
    ))
}
async fn home(State(app): State<App>, User(user): User) -> Result<Json<Value>> {
    let sources: Vec<_> = app
        .store
        .list(&user, "source")?
        .iter()
        .map(|source| {
            let mut source = sources::public(source);
            if let Some(map) = source.as_object_mut() {
                map.remove("text");
            }
            source
        })
        .collect();
    let reports: Vec<_> = crate::report_access::list(&app, &user)?
        .iter()
        .map(|r| {
            let mut r = r.clone();
            r["sourceCount"] = json!(r["sources"].as_array().map(Vec::len).unwrap_or(0));
            r["body"] = json!("");
            r["sources"] = json!([]);
            if let Some(map) = r.as_object_mut() {
                map.remove("claims");
            }
            r
        })
        .collect();
    let conversations:Vec<_>=app.store.list(&user,"conversation")?.iter().map(|c|json!({"id":c["id"],"reportId":c["reportId"],"thoughtId":c["thoughtId"],"updatedAt":c["updatedAt"],"createdAt":c["createdAt"]})).collect();
    Ok(Json(
        json!({"profile":profile::public_profile(&app.store.profile(&user)?),"thoughts":app.store.list(&user,"thought")?.into_iter().filter(|t| t["status"] != "deleted").collect::<Vec<_>>(),"reports":reports,"sources":sources,"jobs":app.store.jobs(&user)?,"conversations":conversations}),
    ))
}
async fn clear(State(app): State<App>, User(user): User) -> Result<StatusCode> {
    let mut conn = app.store.conn()?;
    let tx = conn.transaction()?;
    let files: Vec<String> = {
        let mut stmt=tx.prepare("SELECT json_extract(data,'$._audioPath') FROM records WHERE user_id=? AND kind='source' AND json_extract(data,'$._audioPath') IS NOT NULL")?;
        let rows = stmt.query_map([&user], |r| r.get(0))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
    };
    tx.execute("DELETE FROM records WHERE user_id=?", [&user])?;
    tx.execute("DELETE FROM jobs WHERE user_id=?", [&user])?;
    tx.execute("DELETE FROM receipts WHERE user_id=?", [&user])?;
    tx.execute(
        "UPDATE users SET profile=?,epoch=epoch+1 WHERE id=?",
        rusqlite::params![profile::default_profile().to_string(), user],
    )?;
    tx.commit()?;
    drop(conn);
    for file in files {
        let _ = std::fs::remove_file(app.data_dir.join("audio").join(file));
    }
    Ok(StatusCode::NO_CONTENT)
}
