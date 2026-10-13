use crate::error::{Error, Result};
use argon2::{
    password_hash::{PasswordHash, SaltString},
    Argon2, PasswordHasher, PasswordVerifier,
};
use std::sync::{Arc, OnceLock};
use tokio::sync::Semaphore;

fn gate() -> Arc<Semaphore> {
    static GATE: OnceLock<Arc<Semaphore>> = OnceLock::new();
    GATE.get_or_init(|| Arc::new(Semaphore::new(2))).clone()
}
pub fn validate(password: &str) -> Result<()> {
    if !(12..=128).contains(&password.chars().count()) || password.len() > 512 {
        return Err(Error::bad("密码须为12至128个字符"));
    }
    Ok(())
}
pub async fn hash_password(password: String) -> Result<String> {
    validate(&password)?;
    let permit = gate().try_acquire_owned().map_err(|_| busy())?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let mut salt = [0u8; 16];
        getrandom::fill(&mut salt).map_err(|_| Error::internal())?;
        let salt = SaltString::encode_b64(&salt).map_err(|_| Error::internal())?;
        Argon2::default()
            .hash_password(password.as_bytes(), &salt)
            .map(|v| v.to_string())
            .map_err(|_| Error::internal())
    })
    .await
    .map_err(|_| Error::internal())?
}
pub async fn verify(password: String, encoded: Option<String>) -> Result<bool> {
    if password.len() > 512 {
        return Ok(false);
    }
    let permit = gate().try_acquire_owned().map_err(|_| busy())?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        // The unknown-account path performs the same expensive hash verification.
        let dummy = "$argon2id$v=19$m=19456,t=2,p=1$c29tZXNhbHQxMjM0NTY3OA$R0aZGyZffgEmYZww/GUMSt62fjwfTXsAaQaIZDwCusI";
        let parsed = PasswordHash::new(encoded.as_deref().unwrap_or(dummy))
            .map_err(|_| Error::internal())?;
        Ok(Argon2::default().verify_password(password.as_bytes(), &parsed).is_ok() && encoded.is_some())
    }).await.map_err(|_| Error::internal())?
}
fn busy() -> Error {
    Error::new(
        axum::http::StatusCode::TOO_MANY_REQUESTS,
        "auth_rate_limited",
        "登录请求较多，请稍后再试",
    )
}
