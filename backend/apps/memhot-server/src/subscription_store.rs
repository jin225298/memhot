//! Durable source identity and tenant-owned subscription relations.
use crate::{
    error::{Error, Result},
    App,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

pub const MAX_SUBSCRIPTIONS: i64 = 200;
pub fn migrate(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS subscription_sources(
        id TEXT PRIMARY KEY, scope TEXT NOT NULL, url TEXT NOT NULL, host TEXT NOT NULL,
        catalog_id TEXT, next_at INTEGER NOT NULL DEFAULT 0, lease_token TEXT,
        lease_until INTEGER NOT NULL DEFAULT 0, last_attempt INTEGER, last_success INTEGER,
        failures INTEGER NOT NULL DEFAULT 0, last_new INTEGER NOT NULL DEFAULT 0,
        error TEXT, UNIQUE(scope,url));
      CREATE INDEX IF NOT EXISTS subscription_due ON subscription_sources(next_at,lease_until);
      CREATE TABLE IF NOT EXISTS subscriptions(
        id TEXT PRIMARY KEY, user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        source_id TEXT NOT NULL REFERENCES subscription_sources(id) ON DELETE CASCADE,
        title TEXT NOT NULL, category TEXT NOT NULL, enabled INTEGER NOT NULL DEFAULT 1,
        origin TEXT NOT NULL DEFAULT 'managed', created INTEGER NOT NULL,
        UNIQUE(user_id,source_id));
      CREATE INDEX IF NOT EXISTS subscriptions_user ON subscriptions(user_id,enabled);
      CREATE TABLE IF NOT EXISTS subscription_migrations(
        user_id TEXT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE);
      CREATE TABLE IF NOT EXISTS subscription_items(
        source_id TEXT NOT NULL REFERENCES subscription_sources(id) ON DELETE CASCADE,
        id TEXT NOT NULL, url TEXT NOT NULL, title TEXT NOT NULL, body TEXT NOT NULL,
        published TEXT, discovered TEXT NOT NULL, publication_source TEXT NOT NULL,
        publisher TEXT NOT NULL, first_seen INTEGER NOT NULL, expires_at INTEGER NOT NULL,
        PRIMARY KEY(source_id,id));
      CREATE INDEX IF NOT EXISTS subscription_items_expiry ON subscription_items(expires_at);",
    )?;
    let users = {
        let mut stmt = conn.prepare(
            "SELECT id FROM users WHERE id NOT IN (SELECT user_id FROM subscription_migrations)",
        )?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        rows
    };
    // Startup migration is atomic even when interrupted before all users are imported.
    let tx = conn.unchecked_transaction()?;
    for user in users {
        migrate_legacy_conn(&tx, &user)?;
    }
    tx.commit()?;
    Ok(())
}
pub fn migrate_legacy(app: &App, user: &str) -> Result<()> {
    let mut conn = app.store.conn()?;
    let tx = conn.transaction()?;
    migrate_legacy_conn(&tx, user)?;
    tx.commit()?;
    Ok(())
}
fn migrate_legacy_conn(conn: &Connection, user: &str) -> Result<()> {
    if conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM subscription_migrations WHERE user_id=?)",
        [user],
        |r| r.get::<_, bool>(0),
    )? {
        return Ok(());
    }
    let raw: String = conn
        .query_row("SELECT profile FROM users WHERE id=?", [user], |r| r.get(0))
        .optional()?
        .ok_or_else(Error::auth)?;
    let mut profile: Value = serde_json::from_str(&raw)?;
    for url in profile["feeds"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .take(20)
    {
        if crate::network::validate_url(url).is_ok() {
            add_conn(conn, user, url, None, None, None, "legacy")?;
        }
    }
    profile["feeds"] = json!([]);
    conn.execute(
        "UPDATE users SET profile=? WHERE id=?",
        params![profile.to_string(), user],
    )?;
    conn.execute(
        "INSERT INTO subscription_migrations(user_id) VALUES(?)",
        [user],
    )?;
    Ok(())
}
pub fn has_enabled(app: &App, user: &str) -> Result<bool> {
    migrate_legacy(app, user)?;
    Ok(app.store.conn()?.query_row(
        "SELECT EXISTS(SELECT 1 FROM subscriptions WHERE user_id=? AND enabled=1)",
        [user],
        |r| r.get(0),
    )?)
}
pub fn legacy_feeds(app: &App, user: &str) -> Result<Vec<String>> {
    migrate_legacy(app, user)?;
    let conn = app.store.conn()?;
    let mut stmt=conn.prepare("SELECT s.url,s.catalog_id FROM subscriptions u JOIN subscription_sources s ON s.id=u.source_id WHERE u.user_id=? AND u.origin='legacy' AND u.enabled=1 ORDER BY u.created,u.id")?;
    let rows = stmt.query_map([user], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
    })?;
    // A private URL can contain credentials in any path/query. Only explicit catalog URLs are publishable.
    Ok(rows
        .collect::<std::result::Result<Vec<_>, _>>()?
        .into_iter()
        .filter_map(|(u, _)| {
            crate::subscription_catalog::entries()
                .iter()
                .any(|c| c.url == u)
                .then_some(u)
        })
        .collect())
}
pub fn update_profile(
    app: &App,
    user: &str,
    original: &str,
    p: &Value,
    feeds: Option<&Value>,
) -> Result<()> {
    let mut conn = app.store.conn()?;
    let tx = conn.transaction()?;
    migrate_legacy_conn(&tx, user)?;
    if tx.execute(
        "UPDATE users SET profile=?,name=? WHERE id=? AND profile=?",
        params![
            p.to_string(),
            p["name"].as_str().unwrap_or("我"),
            user,
            original
        ],
    )? == 0
    {
        return Err(crate::topics::conflict());
    }
    if let Some(feeds) = feeds {
        replace_legacy_feeds(&tx, user, feeds)?;
    }
    tx.commit()?;
    Ok(())
}
pub fn replace_legacy_feeds(conn: &Connection, user: &str, feeds: &Value) -> Result<()> {
    let feeds = feeds
        .as_array()
        .filter(|v| v.len() <= 20)
        .ok_or_else(|| Error::bad("最多设置20项"))?;
    // Validate the complete replacement before changing any row.
    for feed in feeds {
        crate::network::validate_url(feed.as_str().ok_or_else(|| Error::bad("订阅网址无效"))?)?;
    }
    conn.execute(
        "DELETE FROM subscriptions WHERE user_id=? AND origin='legacy'",
        [user],
    )?;
    for feed in feeds {
        add_conn(
            conn,
            user,
            feed.as_str().unwrap(),
            None,
            None,
            None,
            "legacy",
        )?;
    }
    prune_orphans(conn)?;
    Ok(())
}
pub fn clear_user(conn: &Connection, user: &str) -> Result<()> {
    conn.execute("DELETE FROM subscriptions WHERE user_id=?", [user])?;
    // Keep the marker: clearing an account must not resurrect the old profile.feeds.
    conn.execute(
        "INSERT OR IGNORE INTO subscription_migrations(user_id) VALUES(?)",
        [user],
    )?;
    prune_orphans(conn)
}
pub(crate) fn prune_orphans(conn: &Connection) -> Result<()> {
    conn.execute("DELETE FROM subscription_sources WHERE NOT EXISTS(SELECT 1 FROM subscriptions WHERE source_id=subscription_sources.id)",[])?;
    conn.execute("UPDATE subscription_sources SET lease_token=NULL,lease_until=0 WHERE NOT EXISTS(SELECT 1 FROM subscriptions WHERE source_id=subscription_sources.id AND enabled=1)",[])?;
    Ok(())
}
pub(crate) fn field(value: Option<&str>, default: &str, max: usize) -> Result<String> {
    let value = value.unwrap_or(default).trim();
    if value.is_empty() || value.chars().count() > max {
        return Err(Error::bad("订阅名称或分类长度无效"));
    }
    Ok(value.to_owned())
}
pub fn add(
    app: &App,
    user: &str,
    url: &str,
    title: Option<&str>,
    category: Option<&str>,
    catalog_id: Option<&str>,
) -> Result<Value> {
    let mut conn = app.store.conn()?;
    let tx = conn.transaction()?;
    migrate_legacy_conn(&tx, user)?;
    let id = add_conn(&tx, user, url, title, category, catalog_id, "managed")?;
    tx.commit()?;
    drop(conn);
    get(app, user, &id)
}
pub(crate) fn add_conn(
    conn: &Connection,
    user: &str,
    raw: &str,
    title: Option<&str>,
    category: Option<&str>,
    catalog_id: Option<&str>,
    origin: &str,
) -> Result<String> {
    if raw.len() > 4096 {
        return Err(Error::bad("订阅网址过长"));
    }
    let mut url = crate::network::validate_url(raw.trim())?;
    url.set_fragment(None);
    let host = url.host_str().unwrap_or_default().to_owned();
    let catalog = match catalog_id {
        Some(id) => Some(
            crate::subscription_catalog::find(id)
                .filter(|c| c.url == url.as_str())
                .ok_or_else(|| Error::bad("订阅目录项无效"))?,
        ),
        None => None,
    };
    let scope = catalog
        .map(|c| format!("catalog:{}", c.id))
        .unwrap_or_else(|| format!("user:{user}"));
    let title = field(title, catalog.map(|c| c.title).unwrap_or(&host), 120)?;
    let category = field(
        category.filter(|s| !s.trim().is_empty()),
        catalog.map(|c| c.category).unwrap_or("其他"),
        80,
    )?;
    let existing:Option<String>=conn.query_row("SELECT u.id FROM subscriptions u JOIN subscription_sources s ON s.id=u.source_id WHERE u.user_id=? AND s.scope=? AND s.url=?",params![user,scope,url.as_str()],|r|r.get(0)).optional()?;
    if let Some(id) = existing {
        // A user explicitly managing a migrated row takes ownership of it; old clients cannot delete it.
        if origin == "managed" {
            conn.execute(
                "UPDATE subscriptions SET origin='managed' WHERE id=? AND user_id=?",
                params![id, user],
            )?;
        }
        return Ok(id);
    }
    let count: i64 = conn.query_row(
        "SELECT count(*) FROM subscriptions WHERE user_id=?",
        [user],
        |r| r.get(0),
    )?;
    if count >= MAX_SUBSCRIPTIONS {
        return Err(Error::bad("最多保存200个订阅来源"));
    }
    let source_id = crate::id();
    conn.execute("INSERT OR IGNORE INTO subscription_sources(id,scope,url,host,catalog_id) VALUES(?,?,?,?,?)",params![source_id,scope,url.as_str(),host,catalog_id])?;
    let source_id: String = conn.query_row(
        "SELECT id FROM subscription_sources WHERE scope=? AND url=?",
        params![scope, url.as_str()],
        |r| r.get(0),
    )?;
    let id = crate::id();
    conn.execute("INSERT INTO subscriptions(id,user_id,source_id,title,category,origin,created) VALUES(?,?,?,?,?,?,?)",params![id,user,source_id,title,category,origin,chrono::Utc::now().timestamp()])?;
    Ok(id)
}
pub fn list(app: &App, user: &str) -> Result<Vec<Value>> {
    migrate_legacy(app, user)?;
    let conn = app.store.conn()?;
    let mut stmt=conn.prepare("SELECT u.id,u.title,s.host,u.category,u.enabled,s.last_attempt,s.last_success,s.next_at,s.lease_until,s.last_new,s.error,s.catalog_id,(SELECT count(*) FROM subscription_items i WHERE i.source_id=s.id AND i.expires_at>?2) FROM subscriptions u JOIN subscription_sources s ON s.id=u.source_id WHERE u.user_id=?1 ORDER BY u.created,u.id")?;
    let at = chrono::Utc::now().timestamp();
    let rows=stmt.query_map(params![user,at],|r| {
        let enabled:bool=r.get(4)?;let last_success:Option<i64>=r.get(6)?;let error:Option<String>=r.get(10)?;let catalog:Option<String>=r.get(11)?;
        // Automatic schedules are >=120s; an earlier future attempt is a manual cooldown.
        let status=if !enabled {"disabled"} else if r.get::<_,i64>(8)?>at {"collecting"} else if r.get::<_,i64>(7)?<=at.max(r.get::<_,Option<i64>>(5)?.unwrap_or(0)+60) {"pending"} else if error.is_some() {"error"} else if last_success.is_some() {"healthy"} else {"pending"};
        Ok(json!({"id":r.get::<_,String>(0)?,"title":r.get::<_,String>(1)?,"host":r.get::<_,String>(2)?,"category":r.get::<_,String>(3)?,"enabled":enabled,"status":status,"lastAttemptAt":timestamp(r.get(5)?),"lastSuccessAt":timestamp(last_success),"nextAttemptAt":if enabled {timestamp(Some(r.get(7)?))}else{None},"lastNewCount":r.get::<_,i64>(9)?,"error":error,"shared":catalog.is_some(),"catalogId":catalog,"itemCount":r.get::<_,i64>(12)?}))
    })?;
    rows.map(|r| r.map_err(Into::into)).collect()
}
pub fn get(app: &App, user: &str, id: &str) -> Result<Value> {
    list(app, user)?
        .into_iter()
        .find(|v| v["id"] == id)
        .ok_or_else(Error::missing)
}
pub(crate) fn timestamp(at: Option<i64>) -> Option<String> {
    at.filter(|at| *at > 0)
        .and_then(|at| chrono::DateTime::from_timestamp(at, 0))
        .map(|at| at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
}
