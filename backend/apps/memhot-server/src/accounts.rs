//! Invitation-only accounts. The existing users.id remains the tenant identity.
use crate::{
    error::{Error, Result},
    hash, id, now, App,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
};

#[path = "accounts_api.rs"]
pub mod api;
#[path = "accounts_password.rs"]
pub mod password;
pub use api::{account, login, register, reset_password, revoke_session, sessions};

pub fn migrate(conn: &Connection) -> Result<()> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS accounts(user_id TEXT PRIMARY KEY REFERENCES users(id),email TEXT NOT NULL UNIQUE,password_hash TEXT NOT NULL,disabled INTEGER NOT NULL DEFAULT 0,created TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS account_invites(code_hash TEXT PRIMARY KEY,email TEXT NOT NULL,claim_user_id TEXT REFERENCES users(id),expires INTEGER NOT NULL,used INTEGER NOT NULL DEFAULT 0,created TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS account_resets(code_hash TEXT PRIMARY KEY,user_id TEXT NOT NULL REFERENCES users(id),expires INTEGER NOT NULL,used INTEGER NOT NULL DEFAULT 0);")?;
    for (column, sql) in [
        (
            "expires",
            "ALTER TABLE devices ADD COLUMN expires INTEGER NOT NULL DEFAULT 0",
        ),
        (
            "session_id",
            "ALTER TABLE devices ADD COLUMN session_id TEXT",
        ),
    ] {
        let exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('devices') WHERE name=?)",
            [column],
            |r| r.get(0),
        )?;
        if !exists {
            conn.execute_batch(sql)?;
        }
    }
    conn.execute(
        "UPDATE devices SET session_id=lower(hex(randomblob(20))) WHERE session_id IS NULL",
        [],
    )?;
    crate::billing::migrate(conn)?;
    crate::public_usage::migrate(conn)
}
pub fn normalize_email(raw: &str) -> Result<String> {
    let email = raw.trim().to_ascii_lowercase();
    let parts: Vec<_> = email.split('@').collect();
    if email.len() > 254
        || !email.is_ascii()
        || parts.len() != 2
        || parts[0].is_empty()
        || parts[0].len() > 64
        || !parts[1].contains('.')
        || parts[1].starts_with('.')
        || parts[1].ends_with('.')
        || email.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return Err(Error::bad("请输入有效邮箱"));
    }
    Ok(email)
}
pub fn auth_error() -> Error {
    Error::new(
        axum::http::StatusCode::UNAUTHORIZED,
        "invalid_credentials",
        "邮箱、密码或邀请信息无效",
    )
}
pub fn rate(app: &App, action: &str, subject: &str) -> Result<()> {
    type Windows = HashMap<String, (i64, u32)>;
    static LIMITS: OnceLock<Mutex<Windows>> = OnceLock::new();
    let mut limits = LIMITS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .map_err(|_| Error::internal())?;
    let at = chrono::Utc::now().timestamp();
    limits.retain(|_, (started, _)| *started > at - 900);
    let namespace = app.data_dir.to_string_lossy();
    let global = format!("{namespace}:global");
    let individual = format!("{namespace}:{action}:{}", hash(subject.as_bytes()));
    for (key, max) in [(global, 120), (individual, 10)] {
        if limits.len() >= 4096 && !limits.contains_key(&key) {
            return Err(rate_error());
        }
        let entry = limits.entry(key).or_insert((at, 0));
        if entry.1 >= max {
            return Err(rate_error());
        }
        entry.1 += 1;
    }
    Ok(())
}
fn rate_error() -> Error {
    Error::new(
        axum::http::StatusCode::TOO_MANY_REQUESTS,
        "auth_rate_limited",
        "尝试次数过多，请15分钟后再试",
    )
}
pub fn interaction_rate(app: &App, user: &str) -> Result<()> {
    static LIMITS: OnceLock<Mutex<HashMap<String, (i64, u32)>>> = OnceLock::new();
    let at = chrono::Utc::now().timestamp();
    let mut limits = LIMITS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .map_err(|_| Error::internal())?;
    limits.retain(|_, (started, _)| *started > at - 60);
    let key = format!("{}:{user}", app.data_dir.to_string_lossy());
    if limits.len() >= 4096 && !limits.contains_key(&key) {
        return Err(rate_error());
    }
    let entry = limits.entry(key).or_insert((at, 0));
    if entry.1 >= 20 {
        return Err(Error::new(
            axum::http::StatusCode::TOO_MANY_REQUESTS,
            "interaction_rate_limited",
            "请求过于频繁，请一分钟后再试",
        ));
    }
    entry.1 += 1;
    Ok(())
}
pub fn create_invite(app: &App, email: &str, user: Option<&str>, days: i64) -> Result<String> {
    let email = normalize_email(email)?;
    if !(1..=30).contains(&days) {
        return Err(Error::bad("邀请有效期须为1至30天"));
    }
    let mut conn = app.store.conn()?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    if tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM accounts WHERE email=?)",
        [&email],
        |r| r.get::<_, bool>(0),
    )? {
        return Err(Error::bad("该邮箱已有账号，请使用管理员重置流程"));
    }
    if let Some(user) = user {
        let available:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM users WHERE id=? AND NOT EXISTS(SELECT 1 FROM accounts WHERE user_id=?))",params![user,user],|r|r.get(0))?;
        if !available {
            return Err(Error::bad("原用户不存在或已被认领"));
        }
        tx.execute(
            "UPDATE account_invites SET used=1 WHERE claim_user_id=? AND used=0",
            [user],
        )?;
    }
    tx.execute(
        "UPDATE account_invites SET used=1 WHERE email=? AND used=0",
        [&email],
    )?;
    let code = id();
    tx.execute("INSERT INTO account_invites(code_hash,email,claim_user_id,expires,created) VALUES(?,?,?,?,?)",params![hash(code.as_bytes()),email,user,chrono::Utc::now().timestamp()+days*86400,now()])?;
    tx.commit()?;
    Ok(code)
}
pub fn create_reset(app: &App, email: &str) -> Result<String> {
    let email = normalize_email(email)?;
    let mut conn = app.store.conn()?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let user: Option<String> = tx
        .query_row(
            "SELECT user_id FROM accounts WHERE email=? AND disabled=0",
            [email],
            |r| r.get(0),
        )
        .optional()?;
    let user = user.ok_or_else(Error::missing)?;
    tx.execute("UPDATE account_resets SET used=1 WHERE user_id=?", [&user])?;
    let code = id();
    tx.execute(
        "INSERT INTO account_resets(code_hash,user_id,expires) VALUES(?,?,?)",
        params![
            hash(code.as_bytes()),
            user,
            chrono::Utc::now().timestamp() + 900
        ],
    )?;
    tx.commit()?;
    Ok(code)
}
pub fn user_json(conn: &Connection, user: &str) -> Result<Value> {
    conn.query_row("SELECT u.name,a.email FROM users u LEFT JOIN accounts a ON a.user_id=u.id WHERE u.id=?",[user],|r|Ok(json!({"id":user,"name":r.get::<_,String>(0)?,"email":r.get::<_,Option<String>>(1)?,"emailVerified":false}))).map_err(Into::into)
}
pub fn session(conn: &Connection, user: &str, name: &str) -> Result<Value> {
    let token = format!("{}{}", id(), id());
    let expires = chrono::Utc::now().timestamp() + 30 * 86400;
    conn.execute("INSERT INTO devices(token_hash,user_id,name,created,expires,session_id) VALUES(?,?,?,?,?,?)",params![hash(token.as_bytes()),user,name,now(),expires,id()])?;
    conn.execute(
        "DELETE FROM devices WHERE user_id=? AND expires>0 AND expires<=?",
        params![user, chrono::Utc::now().timestamp()],
    )?;
    // Bound active devices. Preserve the newly minted session.
    conn.execute("DELETE FROM devices WHERE user_id=? AND token_hash IN (SELECT token_hash FROM devices WHERE user_id=? ORDER BY created DESC,rowid DESC LIMIT -1 OFFSET 12)",params![user,user])?;
    Ok(json!({"token":token,"expiresAt":expires,"user":user_json(conn,user)?}))
}
