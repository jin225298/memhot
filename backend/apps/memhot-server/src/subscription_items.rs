//! Collected summaries are selected through an enabled tenant subscription, never by source ID alone.
use crate::auth::User;
use crate::{
    discovery::Material,
    error::{Error, Result},
    subscription_store, App,
};
use axum::{
    extract::{Path, State},
    Json,
};
use rusqlite::{params, Connection};
use serde_json::{json, Value};
pub const RETENTION_SECONDS: i64 = 30 * 86400;

fn material(r: &rusqlite::Row<'_>) -> rusqlite::Result<Material> {
    let source: String = r.get(6)?;
    Ok(Material {
        url: r.get(0)?,
        title: r.get(1)?,
        text: r.get(2)?,
        published: r.get(3)?,
        discovered: Some(r.get(4)?),
        publisher: r.get(5)?,
        publication_source: if source == "aggregator_submission" {
            "aggregator_submission"
        } else {
            "feed_publication"
        },
        discovered_via: Some(r.get(7)?),
        fulltext: false,
        pdf: None,
    })
}
pub fn materials(app: &App, user: &str) -> Result<Vec<Material>> {
    Ok(personal_materials(app, user, None, 0)?.0)
}
/// Page matching enabled-cache entries without losing later matches to the automatic seed limit.
pub fn search_materials_page(
    app: &App,
    user: &str,
    query: &str,
    offset: usize,
) -> Result<(Vec<Material>, bool)> {
    if !(2..=180).contains(&query.trim().chars().count()) {
        return Err(Error::bad("请输入2至180字的订阅查询词"));
    }
    personal_materials(app, user, Some(query.trim()), offset)
}
fn personal_materials(
    app: &App,
    user: &str,
    query: Option<&str>,
    offset: usize,
) -> Result<(Vec<Material>, bool)> {
    subscription_store::migrate_legacy(app, user)?;
    let conn = app.store.conn()?;
    let mut stmt=conn.prepare("WITH ranked AS (SELECT i.*,s.host,ROW_NUMBER() OVER (PARTITION BY i.source_id ORDER BY julianday(CASE WHEN i.publication_source='aggregator_submission' THEN i.discovered ELSE i.published END) DESC NULLS LAST,i.first_seen DESC,i.id) AS seat FROM subscription_items i JOIN subscription_sources s ON s.id=i.source_id JOIN subscriptions u ON u.source_id=i.source_id WHERE u.user_id=?1 AND u.enabled=1 AND i.expires_at>?2) SELECT url,title,body,published,discovered,publisher,publication_source,host FROM ranked ORDER BY seat,source_id LIMIT ?3")?;
    let rows = stmt.query_map(
        params![
            user,
            chrono::Utc::now().timestamp(),
            if query.is_some() { -1 } else { 200 }
        ],
        material,
    )?;
    let queries = query.map(|q| vec![q.to_owned()]);
    let mut found = Vec::new();
    let mut skipped = 0;
    for row in rows {
        let item = row?;
        if queries
            .as_ref()
            .is_none_or(|q| crate::feed::relevant(&item, q))
        {
            if skipped < offset {
                skipped += 1;
                continue;
            }
            found.push(item);
        }
        if found.len() > 200 {
            break;
        }
    }
    let has_more = found.len() > 200;
    found.truncate(200);
    Ok((found, has_more))
}
/// Fixed public editions may consume only the explicitly curated shared catalog cache.
pub fn public_materials(app: &App) -> Result<Vec<Material>> {
    let conn = app.store.conn()?;
    let mut stmt=conn.prepare("WITH ranked AS (SELECT i.*,s.host,ROW_NUMBER() OVER (PARTITION BY i.source_id ORDER BY julianday(CASE WHEN i.publication_source='aggregator_submission' THEN i.discovered ELSE i.published END) DESC NULLS LAST,i.first_seen DESC,i.id) AS seat FROM subscription_items i JOIN subscription_sources s ON s.id=i.source_id WHERE s.catalog_id IS NOT NULL AND i.expires_at>?1 AND EXISTS(SELECT 1 FROM subscriptions u WHERE u.source_id=s.id AND u.enabled=1)) SELECT url,title,body,published,discovered,publisher,publication_source,host FROM ranked ORDER BY seat,source_id LIMIT 200")?;
    let rows = stmt.query_map([chrono::Utc::now().timestamp()], material)?;
    rows.map(|r| r.map_err(Into::into)).collect()
}
pub async fn list(
    State(app): State<App>,
    User(user): User,
    Path(id): Path<String>,
) -> Result<Json<Value>> {
    let subscription = subscription_store::get(&app, &user, &id)?;
    let conn = app.store.conn()?;
    let mut stmt=conn.prepare("SELECT i.id,i.url,i.title,i.body,i.published,i.discovered,i.publisher FROM subscription_items i JOIN subscriptions u ON u.source_id=i.source_id WHERE u.id=?1 AND u.user_id=?2 AND i.expires_at>?3 ORDER BY julianday(CASE WHEN i.publication_source='aggregator_submission' THEN i.discovered ELSE i.published END) DESC NULLS LAST,i.first_seen DESC,i.id LIMIT 100")?;
    let rows=stmt.query_map(params![id,user,chrono::Utc::now().timestamp()],|r|Ok(json!({"id":r.get::<_,String>(0)?,"url":r.get::<_,String>(1)?,"title":r.get::<_,String>(2)?,"excerpt":crate::clip(&r.get::<_,String>(3)?,400),"publishedAt":r.get::<_,Option<String>>(4)?,"discoveredAt":r.get::<_,String>(5)?,"publisher":r.get::<_,String>(6)?})))?;
    let items = rows.collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(Json(json!({"subscription":subscription,"items":items})))
}
pub(crate) fn persist(conn: &Connection, source: &str, items: &[Material], at: i64) -> Result<i64> {
    let mut added = 0;
    for item in items.iter().take(200) {
        let mut url = match crate::subscription_preview::public_url(&item.url) {
            Ok(url) => url,
            Err(_) => continue,
        };
        url.set_fragment(None);
        let id = crate::hash(url.as_str().as_bytes());
        let discovered = item
            .discovered
            .as_deref()
            .and_then(crate::freshness::publication)
            .map(|date| date.to_rfc3339())
            .unwrap_or_else(|| subscription_store::timestamp(Some(at)).unwrap_or_default());
        let published = item
            .published
            .as_deref()
            .and_then(crate::freshness::publication)
            .map(|date| date.to_rfc3339());
        // Original publication is never synthesized from the collection timestamp, including initial backfills.
        added+=conn.execute("INSERT OR IGNORE INTO subscription_items(source_id,id,url,title,body,published,discovered,publication_source,publisher,first_seen,expires_at) VALUES(?,?,?,?,?,?,?,?,?,?,?)",params![source,id,url.as_str(),crate::clip(&item.title,300),crate::clip(&item.text,12000),published,discovered,item.publication_source,crate::clip(&item.publisher,200),at,at+RETENTION_SECONDS])? as i64;
    }
    // Bound durable cache even for a source publishing thousands of entries a day.
    conn.execute("DELETE FROM subscription_items WHERE source_id=?1 AND id IN (SELECT id FROM subscription_items WHERE source_id=?1 ORDER BY julianday(CASE WHEN publication_source='aggregator_submission' THEN discovered ELSE published END) DESC NULLS LAST,first_seen DESC,id LIMIT -1 OFFSET 1000)",[source])?;
    Ok(added)
}
pub fn retention(app: &App, at: i64) -> Result<()> {
    let conn = app.store.conn()?;
    conn.execute("DELETE FROM subscription_items WHERE expires_at<=?", [at])?;
    subscription_store::prune_orphans(&conn)
}
