//! Day membership selection precedes all filters, facets, counts and pagination.
use super::*;
fn source(scope: &str, bounded: bool) -> String {
    let day = match scope {
        "today" => "d.local_date=?10 AND COALESCE(json_extract(r.data,'$.status'),'open') IN ('open','kept')",
        _ => "(d.local_date<?10 OR (d.local_date=?10 AND json_extract(r.data,'$.status') IN ('dismissed','resolved')))",
    };
    let bounds = if bounded {
        "AND (?5 IS NULL OR d.local_date>=?5) AND (?6 IS NULL OR d.local_date<?6)"
    } else {
        ""
    };
    let selected = if bounded {
        "MAX(d.local_date)"
    } else {
        "d.local_date"
    };
    let group = if bounded {
        "GROUP BY r.user_id,r.id"
    } else {
        ""
    };
    format!("WITH scoped AS (SELECT r.user_id,r.kind,r.id,r.data,{selected} AS created FROM records r JOIN thought_days d ON d.user_id=r.user_id AND d.thought_id=r.id WHERE r.user_id=?1 AND r.kind='thought' AND json_extract(r.data,'$.mergedInto') IS NULL AND {day} {bounds} {group}) ")
}
fn count(
    conn: &Connection,
    user: &str,
    f: &Filters,
    scope: &str,
    today: &str,
) -> Result<(u64, u64)> {
    let sql=format!("{} SELECT COALESCE(SUM(COALESCE(json_extract(data,'$.status'),'open') NOT IN ('dismissed','resolved')),0),COALESCE(SUM(json_extract(data,'$.status') IN ('dismissed','resolved')),0) {}",source(scope,true),FILTER.replace("FROM records","FROM scoped"));
    Ok(conn.query_row(
        &sql,
        params![
            user,
            f.q,
            "all",
            f.category,
            f.start,
            f.end,
            None::<String>,
            None::<String>,
            0,
            today
        ],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?)
}
pub(super) fn list(app: App, user: String, page: Page) -> Result<Json<Value>> {
    let conn = app.store.conn()?;
    let tz = crate::thought_days::timezone(&conn, &user)?;
    let today = crate::thought_days::today(&conn, &user)?;
    let scope = page.scope.as_deref().unwrap_or("today");
    let mut f = filters(&page, tz)?;
    // Scoped date bounds compare calendar membership days, not record creation instants.
    for bound in [&mut f.start, &mut f.end] {
        if let Some(raw) = bound {
            *raw = chrono::DateTime::parse_from_rfc3339(raw)
                .map_err(|_| Error::internal())?
                .with_timezone(&tz)
                .date_naive()
                .to_string();
        }
    }
    let binding = crate::hash(&serde_json::to_vec(
        &json!({"filters":f,"scope":scope,"today":today}),
    )?);
    // Legacy cursors did not bind to a scope; scoped requests require the bound form.
    if let Some(raw) = page.before.as_deref() {
        if matches!(
            serde_json::from_slice::<Cursor>(
                &URL_SAFE_NO_PAD
                    .decode(raw)
                    .map_err(|_| Error::bad("分页位置无效"))?
            ),
            Ok(Cursor::Legacy(_))
        ) {
            return Err(Error::bad("分页位置无效"));
        }
    }
    let cursor = cursor(page.before.as_deref(), &binding)?;
    let limit = page.limit.unwrap_or(50).clamp(1, 100);
    let (ta, tb) = count(&conn, &user, &f, "today", &today)?;
    let (ha, hb) = count(&conn, &user, &f, "history", &today)?;
    let (active, archived) = if scope == "today" { (ta, tb) } else { (ha, hb) };
    let total = match f.status.as_str() {
        "active" => active,
        "archived" => archived,
        _ => active + archived,
    };
    let sql=format!("{} SELECT data,created,id {} AND (?7 IS NULL OR created<?7 OR (created=?7 AND id<?8)) ORDER BY created DESC,id DESC LIMIT ?9",source(scope,true),FILTER.replace("FROM records","FROM scoped"));
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(
        params![
            user,
            f.q,
            f.status,
            f.category,
            f.start,
            f.end,
            cursor.as_ref().map(|c| &c.0),
            cursor.as_ref().map(|c| &c.1),
            limit + 1,
            today
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
            URL_SAFE_NO_PAD.encode(
                serde_json::to_vec(&BoundCursor {
                    created: created.clone(),
                    id: id.clone(),
                    filter: binding,
                })
                .unwrap_or_default(),
            )
        })
    } else {
        None
    };
    let mut items = Vec::new();
    for (raw, date, _) in rows {
        let mut value: Value = serde_json::from_str(&raw)?;
        if value["category"]
            .as_str()
            .is_none_or(|s| s.trim().is_empty())
        {
            value["category"] = json!("未分类");
        }
        crate::thought_days::annotate(&conn, &user, &mut value, Some(&date))?;
        items.push(value);
    }
    let mut categories = BTreeSet::new();
    let mut dates = BTreeSet::new();
    // Facets describe this scope independently of the currently selected filter.
    let sql=format!("{} SELECT COALESCE(NULLIF(TRIM(json_extract(data,'$.category')),''),'未分类'),created FROM scoped WHERE COALESCE(json_extract(data,'$.status'),'open')!='deleted'",source(scope,false));
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(
        params![
            user,
            None::<String>,
            None::<String>,
            None::<String>,
            None::<String>,
            None::<String>,
            None::<String>,
            None::<String>,
            0,
            today
        ],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
    )?;
    for row in rows {
        let (category, date) = row?;
        categories.insert(category);
        dates.insert(date);
    }
    Ok(Json(
        json!({"items":items,"nextCursor":next,"total":total,"todayDate":today,"categories":categories.into_iter().collect::<Vec<_>>(),"dates":dates.into_iter().rev().take(366).collect::<Vec<_>>(),"counts":{"active":active,"archived":archived,"today":ta+tb,"history":ha+hb}}),
    ))
}
