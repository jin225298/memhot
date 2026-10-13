//! Authenticated subscription management. Raw feed URLs never appear in responses.
use crate::{
    auth::User,
    error::{Error, Result},
    subscription_store as store, App,
};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};

pub async fn list(State(app): State<App>, User(user): User) -> Result<Json<Value>> {
    Ok(Json(json!({"subscriptions":store::list(&app,&user)?})))
}
pub async fn create(
    State(app): State<App>,
    User(user): User,
    Json(body): Json<Value>,
) -> Result<Json<Value>> {
    let (url, title, category, catalog) = input(&body)?;
    Ok(Json(store::add(
        &app,
        &user,
        &url,
        title.as_deref(),
        category.as_deref(),
        catalog.as_deref(),
    )?))
}
fn optional_text(body: &Value, key: &str) -> Result<Option<String>> {
    body.get(key)
        .map(|v| {
            v.as_str()
                .map(str::to_owned)
                .ok_or_else(|| Error::bad("订阅字段必须是文本"))
        })
        .transpose()
}
fn input(body: &Value) -> Result<(String, Option<String>, Option<String>, Option<String>)> {
    let map = body
        .as_object()
        .ok_or_else(|| Error::bad("订阅必须是JSON对象"))?;
    if map
        .keys()
        .any(|k| !matches!(k.as_str(), "url" | "title" | "category" | "catalogId"))
    {
        return Err(Error::bad("未知订阅字段"));
    }
    let catalog = optional_text(body, "catalogId")?;
    let url = if let Some(id) = &catalog {
        if body.get("url").is_some() {
            return Err(Error::bad("目录项和自定义网址不能同时指定"));
        }
        crate::subscription_catalog::find(id)
            .ok_or_else(|| Error::bad("订阅目录项不存在"))?
            .url
            .to_owned()
    } else {
        optional_text(body, "url")?.ok_or_else(|| Error::bad("请输入RSS或Atom网址"))?
    };
    Ok((
        url,
        optional_text(body, "title")?,
        optional_text(body, "category")?,
        catalog,
    ))
}
pub async fn import(
    State(app): State<App>,
    User(user): User,
    Json(body): Json<Value>,
) -> Result<Json<Value>> {
    let feeds = body["feeds"]
        .as_array()
        .filter(|v| !v.is_empty() && v.len() <= 100)
        .ok_or_else(|| Error::bad("每次可导入1至100个订阅"))?;
    let entries = feeds.iter().map(input).collect::<Result<Vec<_>>>()?;
    store::migrate_legacy(&app, &user)?;
    let mut conn = app.store.conn()?;
    let tx = conn.transaction()?;
    let mut ids = Vec::new();
    for (url, title, category, catalog) in entries {
        ids.push(store::add_conn(
            &tx,
            &user,
            &url,
            title.as_deref(),
            category.as_deref(),
            catalog.as_deref(),
            "managed",
        )?);
    }
    tx.commit()?;
    drop(conn);
    let subscriptions: Vec<_> = store::list(&app, &user)?
        .into_iter()
        .filter(|v| ids.iter().any(|id| v["id"] == *id))
        .collect();
    Ok(Json(
        json!({"subscriptions":subscriptions,"count":subscriptions.len()}),
    ))
}
pub async fn update(
    State(app): State<App>,
    User(user): User,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>> {
    let map = body
        .as_object()
        .ok_or_else(|| Error::bad("订阅设置必须是JSON对象"))?;
    if map.is_empty()
        || map
            .keys()
            .any(|k| !matches!(k.as_str(), "enabled" | "title" | "category"))
    {
        return Err(Error::bad("订阅设置字段无效"));
    }
    let enabled = map
        .get("enabled")
        .map(|v| {
            v.as_bool()
                .ok_or_else(|| Error::bad("启用状态必须是布尔值"))
        })
        .transpose()?;
    let title = optional_text(&body, "title")?
        .map(|s| store::field(Some(&s), "", 120))
        .transpose()?;
    let category = optional_text(&body, "category")?
        .map(|s| {
            store::field(
                Some(&s)
                    .filter(|s| !s.trim().is_empty())
                    .map(String::as_str),
                "其他",
                80,
            )
        })
        .transpose()?;
    let mut conn = app.store.conn()?;
    let tx = conn.transaction()?;
    let (source, was_enabled): (String, bool) = tx
        .query_row(
            "SELECT source_id,enabled FROM subscriptions WHERE id=? AND user_id=?",
            params![id, user],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?
        .ok_or_else(Error::missing)?;
    tx.execute("UPDATE subscriptions SET enabled=COALESCE(?1,enabled),title=COALESCE(?2,title),category=COALESCE(?3,category),origin='managed' WHERE id=?4 AND user_id=?5",params![enabled,title,category,id,user])?;
    if enabled == Some(true) && !was_enabled {
        tx.execute(
            "UPDATE subscription_sources SET next_at=MAX(COALESCE(last_attempt,0)+60,?) WHERE id=?",
            params![chrono::Utc::now().timestamp(), source],
        )?;
    }
    store::prune_orphans(&tx)?;
    tx.commit()?;
    drop(conn);
    Ok(Json(store::get(&app, &user, &id)?))
}
pub async fn delete(
    State(app): State<App>,
    User(user): User,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    let mut conn = app.store.conn()?;
    let tx = conn.transaction()?;
    if tx.execute(
        "DELETE FROM subscriptions WHERE id=? AND user_id=?",
        params![id, user],
    )? == 0
    {
        return Err(Error::missing());
    }
    store::prune_orphans(&tx)?;
    tx.commit()?;
    Ok(StatusCode::NO_CONTENT)
}
pub async fn refresh(
    State(app): State<App>,
    User(user): User,
    Path(id): Path<String>,
) -> Result<Json<Value>> {
    Ok(Json(enqueue(
        &app,
        &user,
        &id,
        chrono::Utc::now().timestamp(),
    )?))
}
pub fn enqueue(app: &App, user: &str, id: &str, at: i64) -> Result<Value> {
    let mut conn = app.store.conn()?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let row=tx.query_row("SELECT s.id,u.enabled,s.lease_until,s.last_attempt,s.next_at FROM subscriptions u JOIN subscription_sources s ON s.id=u.source_id WHERE u.id=? AND u.user_id=?",params![id,user],|r|Ok((r.get::<_,String>(0)?,r.get::<_,bool>(1)?,r.get::<_,i64>(2)?,r.get::<_,Option<i64>>(3)?,r.get::<_,i64>(4)?))).optional()?.ok_or_else(Error::missing)?;
    if !row.1 {
        return Err(Error::bad("请先启用此订阅"));
    }
    let cooldown = row.3.map(|last| last + 60).unwrap_or(0);
    let coalesced = row.2 > at || row.4 <= at;
    let next = if row.2 > at {
        row.2
    } else if coalesced {
        row.4
    } else {
        at.max(cooldown)
    };
    if row.2 <= at {
        tx.execute(
            "UPDATE subscription_sources SET next_at=? WHERE id=?",
            params![next, row.0],
        )?;
    }
    tx.commit()?;
    Ok(
        json!({"queued":true,"coalesced":coalesced,"nextAttemptAt":store::timestamp(Some(next)),"cooldownUntil":store::timestamp(Some(cooldown)),"id":id}),
    )
}
