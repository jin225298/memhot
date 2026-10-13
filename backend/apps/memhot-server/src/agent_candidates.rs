//! Durable discovery inventory. Only a requested page enters model context.
use super::{state, Scope};
use crate::{
    discovery::Material,
    error::{Error, Result},
    App,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

pub(super) const PAGE_SIZE: usize = 20;
pub(crate) const TTL_SECONDS: i64 = 86400;

pub(crate) fn migrate(conn: &Connection) -> Result<()> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS agent_candidates(
        sequence INTEGER PRIMARY KEY AUTOINCREMENT, audience TEXT NOT NULL,
        run_key TEXT NOT NULL, user_id TEXT REFERENCES users(id) ON DELETE CASCADE,
        source_id TEXT NOT NULL, canonical_url TEXT NOT NULL, data TEXT NOT NULL,
        created INTEGER NOT NULL, expires_at INTEGER NOT NULL,
        UNIQUE(audience,run_key,source_id), UNIQUE(audience,run_key,canonical_url));
      CREATE INDEX IF NOT EXISTS agent_candidates_page ON agent_candidates(audience,run_key,sequence);
      CREATE INDEX IF NOT EXISTS agent_candidates_expiry ON agent_candidates(expires_at);
      CREATE TABLE IF NOT EXISTS agent_candidate_collections(
        id TEXT PRIMARY KEY, audience TEXT NOT NULL, run_key TEXT NOT NULL,
        user_id TEXT REFERENCES users(id) ON DELETE CASCADE, expires_at INTEGER NOT NULL);
      CREATE INDEX IF NOT EXISTS agent_candidate_collections_expiry ON agent_candidate_collections(expires_at);
      CREATE TABLE IF NOT EXISTS agent_candidate_members(
        collection_id TEXT NOT NULL REFERENCES agent_candidate_collections(id) ON DELETE CASCADE,
        candidate_sequence INTEGER NOT NULL REFERENCES agent_candidates(sequence) ON DELETE CASCADE,
        PRIMARY KEY(collection_id,candidate_sequence));
      CREATE TABLE IF NOT EXISTS agent_candidate_cursors(
        token TEXT PRIMARY KEY, audience TEXT NOT NULL, run_key TEXT NOT NULL,
        user_id TEXT REFERENCES users(id) ON DELETE CASCADE, query TEXT NOT NULL,
        after_sequence INTEGER NOT NULL, max_sequence INTEGER NOT NULL,
        expires_at INTEGER NOT NULL, collection_id TEXT REFERENCES agent_candidate_collections(id) ON DELETE CASCADE);
      CREATE INDEX IF NOT EXISTS agent_candidate_cursors_expiry ON agent_candidate_cursors(expires_at);")?;
    let has_collection: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info('agent_candidate_cursors') WHERE name='collection_id')", [], |r| r.get(0))?;
    if !has_collection {
        conn.execute_batch("ALTER TABLE agent_candidate_cursors ADD COLUMN collection_id TEXT REFERENCES agent_candidate_collections(id) ON DELETE CASCADE;")?;
    }
    Ok(())
}
fn audience(scope: &Scope) -> String {
    match scope {
        Scope::Private { user, epoch, .. } => json!(["private", user, epoch]).to_string(),
        Scope::Public { catalog_id, .. } => json!(["public", catalog_id]).to_string(),
    }
}
fn owner(scope: &Scope) -> Option<&str> {
    match scope {
        Scope::Private { user, .. } => Some(user),
        Scope::Public { .. } => None,
    }
}
// Recheck while holding the write/read transaction: a concurrent account clear
// must not let a stale run repopulate the durable pool after its epoch changed.
fn authorize(conn: &Connection, scope: &Scope) -> Result<()> {
    if let Scope::Private {
        user,
        epoch,
        job,
        references,
        ..
    } = scope
    {
        let current: Option<i64> = conn
            .query_row("SELECT epoch FROM users WHERE id=?", [user], |r| r.get(0))
            .optional()?;
        if current != Some(*epoch) {
            return Err(Error::missing());
        }
        if let Some(job) = job {
            if job.user != *user || !crate::research::job_active(conn, job)? {
                return Err(Error::missing());
            }
        }
        for (kind, id) in references {
            let exists: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM records WHERE user_id=? AND kind=? AND id=?)",
                params![user, kind, id],
                |r| r.get(0),
            )?;
            if !exists {
                return Err(Error::missing());
            }
        }
    }
    Ok(())
}
pub(crate) fn clear_user(conn: &Connection, user: &str) -> Result<()> {
    migrate(conn)?;
    conn.execute(
        "DELETE FROM agent_candidate_cursors WHERE user_id=?",
        [user],
    )?;
    conn.execute(
        "DELETE FROM agent_candidate_collections WHERE user_id=?",
        [user],
    )?;
    conn.execute("DELETE FROM agent_candidates WHERE user_id=?", [user])?;
    Ok(())
}
pub(crate) fn retention(app: &App, at: i64) -> Result<()> {
    let conn = app.store.conn()?;
    migrate(&conn)?;
    conn.execute(
        "DELETE FROM agent_candidate_cursors WHERE expires_at<=?",
        [at],
    )?;
    conn.execute(
        "DELETE FROM agent_candidate_collections WHERE expires_at<=?",
        [at],
    )?;
    conn.execute("DELETE FROM agent_candidates WHERE expires_at<=?", [at])?;
    Ok(())
}
pub(super) fn store(app: &App, scope: &Scope, purpose: &str, materials: &[Material]) -> Result<()> {
    scope.check(app)?;
    let mut conn = app.store.conn()?;
    migrate(&conn)?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    authorize(&tx, scope)?;
    let at = chrono::Utc::now().timestamp();
    tx.execute("DELETE FROM agent_candidates WHERE expires_at<=?", [at])?;
    let audience = audience(scope);
    let run = scope.key(purpose);
    for m in materials {
        if crate::subscription_preview::public_url(&m.url).is_err() {
            continue;
        }
        let canonical = crate::freshness::canonical(&m.url);
        // Keep the first URL and evidence ID across tracking-link duplicates.
        // An inventory entry is discovery metadata, never an admission to evidence.
        let id = crate::hash(m.url.as_bytes())[..24].to_owned();
        let mut m = m.clone();
        m.title = crate::clip(&m.title, 300);
        m.text = crate::clip(&m.text, 64000);
        m.publisher = crate::clip(&m.publisher, 200);
        m.published = m.published.map(|v| crate::clip(&v, 100));
        m.discovered = m.discovered.map(|v| crate::clip(&v, 100));
        m.discovered_via = m.discovered_via.map(|v| crate::clip(&v, 4096));
        tx.execute("INSERT OR IGNORE INTO agent_candidates(audience,run_key,user_id,source_id,canonical_url,data,created,expires_at) VALUES(?,?,?,?,?,?,?,?)",
            params![audience,run,owner(scope),id,canonical,state::encode(&m).to_string(),at,at+TTL_SECONDS])?;
    }
    tx.commit()?;
    Ok(())
}
/// A search response pages only its own result collection. Previously discovered
/// canonical duplicates remain members with their original evidence identity.
pub(super) fn store_and_page(
    app: &App,
    scope: &Scope,
    purpose: &str,
    materials: &[Material],
) -> Result<Value> {
    store(app, scope, purpose, materials)?;
    let collection = crate::id();
    {
        let mut conn = app.store.conn()?;
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        authorize(&tx, scope)?;
        let audience = audience(scope);
        let run = scope.key(purpose);
        let at = chrono::Utc::now().timestamp();
        tx.execute("INSERT INTO agent_candidate_collections(id,audience,run_key,user_id,expires_at) VALUES(?,?,?,?,?)", params![collection,audience,run,owner(scope),at+TTL_SECONDS])?;
        for m in materials {
            if crate::subscription_preview::public_url(&m.url).is_err() {
                continue;
            }
            tx.execute("INSERT OR IGNORE INTO agent_candidate_members(collection_id,candidate_sequence) SELECT ?1,sequence FROM agent_candidates WHERE audience=?2 AND run_key=?3 AND canonical_url=?4 AND expires_at>?5", params![collection,audience,run,crate::freshness::canonical(&m.url),at])?;
        }
        tx.commit()?;
    }
    page(app, scope, purpose, &json!({"collectionId":collection}))
}
fn decode(raw: &str) -> Result<Material> {
    state::decode(&serde_json::from_str(raw)?).ok_or_else(Error::internal)
}
pub(super) fn lookup(app: &App, scope: &Scope, purpose: &str, source_id: &str) -> Result<Material> {
    scope.check(app)?;
    let conn = app.store.conn()?;
    migrate(&conn)?;
    let tx = conn.unchecked_transaction()?;
    authorize(&tx, scope)?;
    let raw: Option<String> = tx.query_row("SELECT data FROM agent_candidates WHERE audience=? AND run_key=? AND source_id=? AND expires_at>?",
        params![audience(scope),scope.key(purpose),source_id,chrono::Utc::now().timestamp()], |r| r.get(0)).optional()?;
    decode(&raw.ok_or_else(Error::missing)?)
}
fn query_input(args: &Value) -> Result<Option<String>> {
    let map = args
        .as_object()
        .ok_or_else(|| Error::bad("候选列表参数无效"))?;
    if map
        .keys()
        .any(|k| !matches!(k.as_str(), "query" | "cursor" | "collectionId"))
    {
        return Err(Error::bad("候选列表参数无效"));
    }
    map.get("query")
        .map(|v| {
            v.as_str()
                .filter(|q| q.chars().count() <= 180)
                .map(|q| q.trim().to_owned())
                .ok_or_else(|| Error::bad("筛选词最多180字"))
        })
        .transpose()
}
pub(super) fn page(app: &App, scope: &Scope, purpose: &str, args: &Value) -> Result<Value> {
    scope.check(app)?;
    let query = query_input(args)?;
    let cursor = args
        .get("cursor")
        .map(|v| {
            v.as_str()
                .filter(|s| s.len() == 40)
                .ok_or_else(|| Error::bad("候选分页标识无效"))
        })
        .transpose()?;
    let collection = args
        .get("collectionId")
        .map(|v| {
            v.as_str()
                .filter(|s| s.len() == 40)
                .map(str::to_owned)
                .ok_or_else(|| Error::bad("候选集合标识无效"))
        })
        .transpose()?;
    let mut conn = app.store.conn()?;
    migrate(&conn)?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    authorize(&tx, scope)?;
    let audience = audience(scope);
    let run = scope.key(purpose);
    let at = chrono::Utc::now().timestamp();
    let (query, after, max, mut expires, collection) = if let Some(cursor) = cursor {
        let row: Option<(String, i64, i64, i64, Option<String>)> = tx.query_row("SELECT query,after_sequence,max_sequence,expires_at,collection_id FROM agent_candidate_cursors WHERE token=? AND audience=? AND run_key=? AND expires_at>?",
            params![cursor,audience,run,at], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional()?;
        let row = row.ok_or_else(|| Error::bad("候选分页已失效，请重新列出候选"))?;
        if query.as_ref().is_some_and(|q| q != &row.0) {
            return Err(Error::bad("分页标识与筛选词不匹配"));
        }
        if collection
            .as_ref()
            .is_some_and(|c| row.4.as_ref() != Some(c))
        {
            return Err(Error::bad("分页标识与候选集合不匹配"));
        }
        row
    } else {
        let max: i64 = tx.query_row("SELECT COALESCE(MAX(sequence),0) FROM agent_candidates WHERE audience=? AND run_key=? AND expires_at>?", params![audience,run,at], |r| r.get(0))?;
        (
            query.unwrap_or_default(),
            0,
            max,
            at + TTL_SECONDS,
            collection,
        )
    };
    if let Some(collection) = &collection {
        let until: Option<i64> = tx.query_row("SELECT expires_at FROM agent_candidate_collections WHERE id=? AND audience=? AND run_key=? AND expires_at>?", params![collection,audience,run,at], |r| r.get(0)).optional()?;
        expires = expires.min(until.ok_or_else(|| Error::bad("候选集合已失效或无权访问"))?);
    }
    // instr gives literal substring matching: percent/underscore/quotes never
    // become wildcards or SQL. Filtering happens over the entire durable pool.
    let total: i64 = tx.query_row("SELECT count(*) FROM agent_candidates WHERE audience=?1 AND run_key=?2 AND expires_at>?3 AND sequence<=?4 AND (?5='' OR instr(lower(json_extract(data,'$.title') || ' ' || json_extract(data,'$.text')),lower(?5))>0) AND (?6 IS NULL OR EXISTS(SELECT 1 FROM agent_candidate_members WHERE collection_id=?6 AND candidate_sequence=agent_candidates.sequence))",
        params![audience,run,at,max,query,collection], |r| r.get(0))?;
    let rows = {
        let mut stmt = tx.prepare("SELECT sequence,source_id,data FROM agent_candidates WHERE audience=?1 AND run_key=?2 AND expires_at>?3 AND sequence<=?4 AND sequence>?5 AND (?6='' OR instr(lower(json_extract(data,'$.title') || ' ' || json_extract(data,'$.text')),lower(?6))>0) AND (?8 IS NULL OR EXISTS(SELECT 1 FROM agent_candidate_members WHERE collection_id=?8 AND candidate_sequence=agent_candidates.sequence)) ORDER BY sequence LIMIT ?7")?;
        let rows = stmt
            .query_map(
                params![
                    audience,
                    run,
                    at,
                    max,
                    after,
                    query,
                    (PAGE_SIZE + 1) as i64,
                    collection
                ],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                },
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        rows
    };
    let has_more = rows.len() > PAGE_SIZE;
    let mut sources = Vec::new();
    let mut last = after;
    for (sequence, id, raw) in rows.into_iter().take(PAGE_SIZE) {
        let m = decode(&raw)?;
        let mut source = m.evidence();
        source["sourceId"] = json!(id);
        source["excerpt"] = json!(crate::clip(&m.text, 320));
        source["candidateOnly"] = json!(true);
        sources.push(source);
        last = sequence;
    }
    let next = if has_more {
        // Repeated reads of a page return the same cursor, bounding replay rows.
        let token = crate::hash(
            json!([audience, run, query, last, max, expires, collection])
                .to_string()
                .as_bytes(),
        )[..40]
            .to_owned();
        tx.execute("INSERT OR IGNORE INTO agent_candidate_cursors(token,audience,run_key,user_id,query,after_sequence,max_sequence,expires_at,collection_id) VALUES(?,?,?,?,?,?,?,?,?)", params![token,audience,run,owner(scope),query,last,max,expires,collection])?;
        Some(token)
    } else {
        None
    };
    tx.commit()?;
    Ok(
        json!({"sources":sources,"totalCandidates":total,"nextCursor":next,"hasMore":has_more,"collectionId":collection}),
    )
}

#[cfg(test)]
#[path = "agent_candidates_tests.rs"]
mod tests;
