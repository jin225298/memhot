use super::*;
use crate::{auth::User, profile};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Register {
    email: String,
    password: String,
    invite_code: String,
    name: String,
    device_name: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Login {
    email: String,
    password: String,
    device_name: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Reset {
    email: String,
    password: String,
    reset_code: String,
}
fn device(name: &str) -> Result<&str> {
    let name = name.trim();
    if name.is_empty() || name.len() > 200 {
        return Err(Error::bad("设备名称须为1至200字节"));
    }
    Ok(name)
}
pub async fn register(State(app): State<App>, Json(input): Json<Register>) -> Result<Json<Value>> {
    let email = normalize_email(&input.email)?;
    rate(&app, "register", &email)?;
    device(&input.device_name)?;
    if input.name.trim().is_empty()
        || input.name.chars().count() > 80
        || input.invite_code.len() > 128
    {
        return Err(Error::bad("注册信息格式无效"));
    }
    let encoded = password::hash_password(input.password).await?;
    let mut conn = app.store.conn()?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let invitation:Option<(Option<String>,i64)>=tx.query_row("SELECT claim_user_id,test_credits FROM account_invites WHERE code_hash=? AND email=? AND used=0 AND expires>?",params![hash(input.invite_code.trim().as_bytes()),email,chrono::Utc::now().timestamp()],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    let (claim, test_credits) = invitation.ok_or_else(auth_error)?;
    let new_user = claim.is_none();
    if tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM accounts WHERE email=?)",
        [&email],
        |r| r.get::<_, bool>(0),
    )? {
        return Err(auth_error());
    }
    let user = if let Some(user) = claim {
        if tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM accounts WHERE user_id=?)",
            [&user],
            |r| r.get::<_, bool>(0),
        )? {
            return Err(auth_error());
        }
        // Bind the existing tenant without replacing its profile, records, or jobs.
        user
    } else {
        let user = id();
        let mut p = profile::default_profile();
        p["name"] = json!(input.name.trim());
        tx.execute(
            "INSERT INTO users(id,name,profile) VALUES(?,?,?)",
            params![user, input.name.trim(), p.to_string()],
        )?;
        user
    };
    tx.execute(
        "INSERT INTO accounts(user_id,email,password_hash,created) VALUES(?,?,?,?)",
        params![user, email, encoded, now()],
    )?;
    tx.execute(
        "UPDATE account_invites SET used=1 WHERE code_hash=?",
        [hash(input.invite_code.trim().as_bytes())],
    )?;
    tx.execute("DELETE FROM pairs WHERE user_id=?", [&user])?;
    tx.execute("DELETE FROM devices WHERE user_id=?", [&user])?;
    crate::billing::ensure_wallet(&tx, &user)?;
    if new_user {
        super::invitation_credits::apply(&tx, &user, test_credits)?;
    }
    let out = session(&tx, &user, &input.device_name)?;
    tx.commit()?;
    Ok(Json(out))
}
pub async fn login(State(app): State<App>, Json(input): Json<Login>) -> Result<Json<Value>> {
    let email = normalize_email(&input.email)?;
    rate(&app, "login", &email)?;
    device(&input.device_name)?;
    let found: Option<(String, String)> = app
        .store
        .conn()?
        .query_row(
            "SELECT user_id,password_hash FROM accounts WHERE email=? AND disabled=0",
            [email],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    if !password::verify(input.password, found.as_ref().map(|v| v.1.clone())).await? {
        return Err(auth_error());
    }
    let (user, hash) = found.ok_or_else(auth_error)?;
    let mut conn = app.store.conn()?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let current: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM accounts WHERE user_id=? AND password_hash=? AND disabled=0)",
        params![user, hash],
        |r| r.get(0),
    )?;
    if !current {
        return Err(auth_error());
    }
    let out = session(&tx, &user, &input.device_name)?;
    tx.commit()?;
    Ok(Json(out))
}
pub async fn reset_password(
    State(app): State<App>,
    Json(input): Json<Reset>,
) -> Result<StatusCode> {
    let email = normalize_email(&input.email)?;
    rate(&app, "reset", &email)?;
    if input.reset_code.len() > 128 {
        return Err(auth_error());
    }
    let encoded = password::hash_password(input.password).await?;
    let mut conn = app.store.conn()?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let user:Option<String>=tx.query_row("SELECT r.user_id FROM account_resets r JOIN accounts a ON a.user_id=r.user_id WHERE r.code_hash=? AND a.email=? AND a.disabled=0 AND r.used=0 AND r.expires>?",params![hash(input.reset_code.trim().as_bytes()),email,chrono::Utc::now().timestamp()],|r|r.get(0)).optional()?;
    let user = user.ok_or_else(auth_error)?;
    tx.execute(
        "UPDATE accounts SET password_hash=? WHERE user_id=?",
        params![encoded, user],
    )?;
    tx.execute("UPDATE account_resets SET used=1 WHERE user_id=?", [&user])?;
    tx.execute("DELETE FROM devices WHERE user_id=?", [&user])?;
    tx.execute("DELETE FROM pairs WHERE user_id=?", [&user])?;
    tx.commit()?;
    Ok(StatusCode::NO_CONTENT)
}
pub async fn account(State(app): State<App>, User(user): User) -> Result<Json<Value>> {
    Ok(Json(crate::billing::account(&app, &user)?))
}
pub async fn sessions(State(app): State<App>, User(user): User) -> Result<Json<Value>> {
    let conn = app.store.conn()?;
    let mut stmt=conn.prepare("SELECT session_id,name,created,expires FROM devices WHERE user_id=? AND (expires=0 OR expires>?) ORDER BY created DESC")?;
    let rows=stmt.query_map(params![user,chrono::Utc::now().timestamp()],|r|Ok(json!({"id":r.get::<_,String>(0)?,"name":r.get::<_,String>(1)?,"createdAt":r.get::<_,String>(2)?,"expiresAt":r.get::<_,i64>(3)?})))?;
    Ok(Json(
        json!({"sessions":rows.collect::<std::result::Result<Vec<_>,_>>()?}),
    ))
}
pub async fn revoke_session(
    State(app): State<App>,
    User(user): User,
    Path(key): Path<String>,
) -> Result<StatusCode> {
    if app.store.conn()?.execute(
        "DELETE FROM devices WHERE user_id=? AND session_id=?",
        params![user, key],
    )? == 0
    {
        return Err(Error::missing());
    }
    Ok(StatusCode::NO_CONTENT)
}
