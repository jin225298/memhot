use crate::{
    auth::User,
    error::{Error, Result},
    subscription_catalog, subscription_opml, subscription_preview,
};
use axum::{http::StatusCode, Json};
use serde::Deserialize;
use serde_json::Value;
use std::{
    collections::HashSet,
    sync::{Mutex, OnceLock},
};
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreviewInput {
    pub url: Option<String>,
    pub catalog_id: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpmlInput {
    pub xml: String,
}
static ACTIVE: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
struct PreviewGuard(String);
impl PreviewGuard {
    fn acquire(user: &str) -> Result<Self> {
        let mut active = ACTIVE
            .get_or_init(Mutex::default)
            .lock()
            .map_err(|_| Error::internal())?;
        if active.contains(user) || active.len() >= 16 {
            return Err(Error::new(
                StatusCode::TOO_MANY_REQUESTS,
                "preview_busy",
                "已有预览正在进行，请稍后再试",
            ));
        }
        active.insert(user.to_owned());
        Ok(Self(user.to_owned()))
    }
}
impl Drop for PreviewGuard {
    fn drop(&mut self) {
        if let Some(active) = ACTIVE.get() {
            if let Ok(mut active) = active.lock() {
                active.remove(&self.0);
            }
        }
    }
}
pub async fn preview(User(user): User, Json(input): Json<PreviewInput>) -> Result<Json<Value>> {
    let (url, catalog_id) = match (input.url, input.catalog_id) {
        (Some(url), None) => (url, None),
        (None, Some(id)) => {
            let source =
                subscription_catalog::find(&id).ok_or_else(|| Error::bad("推荐来源不存在"))?;
            (source.url.to_owned(), Some(id))
        }
        _ => return Err(Error::bad("请提供订阅网址或推荐来源 ID，二者只需选择一项")),
    };
    let _guard = PreviewGuard::acquire(&user)?;
    let mut result = subscription_preview::preview(&url).await?;
    if let Some(id) = catalog_id {
        if let Some(candidates) = result["candidates"].as_array_mut() {
            for candidate in candidates {
                candidate["catalogId"] = Value::String(id.clone());
            }
        }
    }
    Ok(Json(result))
}
pub async fn opml_preview(User(user): User, Json(input): Json<OpmlInput>) -> Result<Json<Value>> {
    let _guard = PreviewGuard::acquire(&user)?;
    Ok(Json(subscription_opml::preview(&input.xml)?))
}
pub async fn catalog(User(_): User) -> Json<Value> {
    Json(subscription_catalog::sources())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn concurrent_preview_is_bounded_and_guard_releases_on_drop() {
        let first = PreviewGuard::acquire("preview-test-user").unwrap();
        assert!(PreviewGuard::acquire("preview-test-user").is_err());
        drop(first);
        assert!(PreviewGuard::acquire("preview-test-user").is_ok());
    }
    #[test]
    fn input_rejects_unknown_credentials_fields() {
        assert!(serde_json::from_value::<PreviewInput>(
            serde_json::json!({"url":"https://example.com", "password":"secret"})
        )
        .is_err());
        assert!(serde_json::from_value::<OpmlInput>(
            serde_json::json!({"xml":"", "fetchAll":true})
        )
        .is_err());
    }
}
