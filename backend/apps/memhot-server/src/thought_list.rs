use crate::{
    auth::User,
    error::{Error, Result},
    App,
};
use axum::{
    extract::{Query, State},
    Json,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::Deserialize;
use serde_json::{json, Value};
#[derive(Deserialize)]
pub struct Page {
    before: Option<String>,
    limit: Option<u32>,
}
pub async fn list(
    State(app): State<App>,
    User(user): User,
    Query(page): Query<Page>,
) -> Result<Json<Value>> {
    let limit = page.limit.unwrap_or(50).clamp(1, 100);
    let cursor: Option<(String, String)> = page
        .before
        .map(|s| {
            if s.len() > 512 {
                return Err(Error::bad("分页位置无效"));
            }
            let bytes = URL_SAFE_NO_PAD
                .decode(s)
                .map_err(|_| Error::bad("分页位置无效"))?;
            let value: (String, String) =
                serde_json::from_slice(&bytes).map_err(|_| Error::bad("分页位置无效"))?;
            if value.0.len() > 64 || value.1.len() > 128 {
                return Err(Error::bad("分页位置无效"));
            }
            Ok(value)
        })
        .transpose()?;
    let conn = app.store.conn()?;
    let mut stmt=conn.prepare("SELECT data,created,id FROM records WHERE user_id=?1 AND kind='thought' AND COALESCE(json_extract(data,'$.status'),'open')!='deleted' AND (?2 IS NULL OR created<?2 OR (created=?2 AND id<?3)) ORDER BY created DESC,id DESC LIMIT ?4")?;
    let rows = stmt.query_map(
        rusqlite::params![
            user,
            cursor.as_ref().map(|x| &x.0),
            cursor.as_ref().map(|x| &x.1),
            limit + 1
        ],
        |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        },
    )?;
    let mut rows = rows.collect::<std::result::Result<Vec<_>, _>>()?;
    let more = rows.len() > limit as usize;
    rows.truncate(limit as usize);
    let next = if more {
        rows.last().map(|(_, created, id)| {
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&(created, id)).unwrap_or_default())
        })
    } else {
        None
    };
    let items = rows
        .into_iter()
        .map(|(raw, _, _)| serde_json::from_str::<Value>(&raw))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(Json(json!({"items":items,"nextCursor":next})))
}
