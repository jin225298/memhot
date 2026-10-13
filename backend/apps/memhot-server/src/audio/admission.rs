use crate::{auth::User, error::Result, models, App};
use axum::{extract::FromRequestParts, http::request::Parts};

/// Authentication and non-waiting admission happen before Axum extracts body bytes.
pub struct AudioAdmission {
    pub(super) user: String,
    pub(super) _permit: models::ModelPermits,
}
impl FromRequestParts<App> for AudioAdmission {
    type Rejection = crate::error::Error;
    async fn from_request_parts(parts: &mut Parts, app: &App) -> Result<Self> {
        let User(user) = User::from_request_parts(parts, app).await?;
        let _permit = models::asr_permit(app, &user)?;
        Ok(Self { user, _permit })
    }
}
