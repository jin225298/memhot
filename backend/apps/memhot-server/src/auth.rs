use crate::{
    error::{Error, Result},
    hash, id, now, profile, App,
};
use axum::{
    extract::{FromRequestParts, State},
    http::request::Parts,
    Json,
};
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Clone, Debug)]
pub struct User(pub String);
impl FromRequestParts<App> for User {
    type Rejection = Error;
    async fn from_request_parts(parts: &mut Parts, state: &App) -> Result<Self> {
        let token = parts
            .headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .filter(|v| v.len() >= 32 && v.len() <= 256)
            .ok_or_else(Error::auth)?;
        let user: Option<String> = state
            .store
            .conn()?
            .query_row(
                "SELECT d.user_id FROM devices d LEFT JOIN accounts a ON a.user_id=d.user_id WHERE d.token_hash=? AND (d.expires>? OR (d.expires=0 AND a.user_id IS NULL)) AND COALESCE(a.disabled,0)=0",
                params![hash(token.as_bytes()),chrono::Utc::now().timestamp()],
                |r| r.get(0),
            )
            .optional()?;
        Ok(Self(user.ok_or_else(Error::auth)?))
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Pair {
    code: String,
    device_name: String,
}
pub async fn pair(State(app): State<App>, Json(input): Json<Pair>) -> Result<Json<Value>> {
    if input.code.len() > 128 || input.device_name.len() > 200 {
        return Err(Error::bad("配对信息过长"));
    }
    let mut conn = app.store.conn()?;
    let tx = conn.transaction()?;
    let user:Option<(String,String)>=tx.query_row("SELECT p.user_id,u.name FROM pairs p JOIN users u ON u.id=p.user_id WHERE p.code_hash=? AND p.expires>? AND NOT EXISTS(SELECT 1 FROM accounts a WHERE a.user_id=p.user_id)",params![hash(input.code.trim().as_bytes()),chrono::Utc::now().timestamp()],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    let (uid, name) = user.ok_or_else(Error::auth)?;
    let token = format!("{}{}", id(), id());
    tx.execute(
        "DELETE FROM pairs WHERE code_hash=?",
        [hash(input.code.trim().as_bytes())],
    )?;
    tx.execute(
        "INSERT INTO devices(token_hash,user_id,name,created,session_id) VALUES(?,?,?,?,?)",
        params![hash(token.as_bytes()), uid, input.device_name, now(), id()],
    )?;
    tx.commit()?;
    Ok(Json(json!({"token":token,"user":{"id":uid,"name":name}})))
}
pub fn create_pair(app: &App, name: &str, user: Option<&str>) -> Result<(String, String)> {
    let mut conn = app.store.conn()?;
    let tx = conn.transaction()?;
    let uid = if let Some(uid) = user {
        if tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM accounts WHERE user_id=?)",
            [uid],
            |r| r.get::<_, bool>(0),
        )? {
            return Err(Error::bad("该用户已启用邮箱登录，不能再使用私有配对"));
        }
        if !tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM users WHERE id=?)",
            [uid],
            |r| r.get::<_, bool>(0),
        )? {
            return Err(Error::missing());
        }
        uid.to_owned()
    } else {
        let uid = id();
        let mut p = profile::default_profile();
        p["name"] = json!(crate::clip(name, 80));
        tx.execute(
            "INSERT INTO users(id,name,profile) VALUES(?,?,?)",
            params![uid, name, p.to_string()],
        )?;
        uid
    };
    let code = id();
    tx.execute(
        "INSERT INTO pairs(code_hash,user_id,expires) VALUES(?,?,?)",
        params![
            hash(code.as_bytes()),
            uid,
            chrono::Utc::now().timestamp() + 900
        ],
    )?;
    tx.execute(
        "DELETE FROM pairs WHERE expires<=?",
        [chrono::Utc::now().timestamp()],
    )?;
    tx.commit()?;
    Ok((uid, code))
}

pub async fn revoke_device(
    State(app): State<App>,
    User(user): User,
    headers: axum::http::HeaderMap,
) -> Result<axum::http::StatusCode> {
    let token = headers
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .ok_or_else(Error::auth)?;
    app.store.conn()?.execute(
        "DELETE FROM devices WHERE user_id=? AND token_hash=?",
        params![user, hash(token.as_bytes())],
    )?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}
