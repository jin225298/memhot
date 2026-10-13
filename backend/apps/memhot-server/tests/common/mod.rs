use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use memhot_server::{api, auth, id, App};
use serde_json::{json, Value};
use tower::ServiceExt;
pub struct Fixture {
    pub app: App,
}
impl Fixture {
    pub fn new() -> Self {
        Self {
            app: App::open(std::env::temp_dir().join(format!("memhot-test-{}", id()))).unwrap(),
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(self.app.data_dir.as_ref());
    }
}
pub async fn request(
    app: &App,
    method: &str,
    path: &str,
    token: &str,
    input: Option<Value>,
) -> (StatusCode, Value) {
    let mut request = Request::builder().method(method).uri(path);
    if !token.is_empty() {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let body = if let Some(input) = input {
        request = request.header("content-type", "application/json");
        Body::from(input.to_string())
    } else {
        Body::empty()
    };
    let response = api::router(app.clone())
        .oneshot(request.body(body).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}
pub async fn user(app: &App, name: &str) -> (String, String) {
    let (uid, code) = auth::create_pair(app, name, None).unwrap();
    let (status, body) = request(
        app,
        "POST",
        "/api/v1/pair",
        "",
        Some(json!({"code":code,"deviceName":"test"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        request(
            app,
            "POST",
            "/api/v1/pair",
            "",
            Some(json!({"code":code,"deviceName":"again"}))
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    (uid, body["token"].as_str().unwrap().to_owned())
}
pub async fn note(app: &App, token: &str, client: &str) -> Value {
    let (status,body)=request(app,"POST","/api/v1/sources",token,Some(json!({"title":"有关学习的问题","kind":"note","text":"我想知道怎样更有效地学习","clientId":client}))).await;
    assert_eq!(status, StatusCode::OK);
    body
}
