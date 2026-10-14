use crate::{
    concurrency::Lane,
    db::{Job, Store},
    error::Result,
};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};

pub(crate) fn migrate(connection: &Connection) -> Result<()> {
    let tx = rusqlite::Transaction::new_unchecked(connection, TransactionBehavior::Immediate)?;
    let exists: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='scheduler_lane_turns')",
        [], |r| r.get(0),
    )?;
    if !exists {
        tx.execute_batch(
            "CREATE TABLE scheduler_lane_turns(
                user_id TEXT NOT NULL REFERENCES users(id),lane TEXT NOT NULL,last_turn INTEGER NOT NULL,
                PRIMARY KEY(user_id,lane));
            INSERT INTO scheduler_lane_turns(user_id,lane,last_turn)
                SELECT user_id,lane,last_turn FROM scheduler_turns CROSS JOIN
                (SELECT 'translation' AS lane UNION ALL SELECT 'digest' UNION ALL SELECT 'research');",
        )?;
    }
    tx.commit()?;
    Ok(())
}

pub(crate) fn claim(store: &Store, lane: Option<Lane>) -> Result<Option<Job>> {
    let lane = match lane {
        Some(Lane::Chat) => return Ok(None), // Chat is an HTTP request, never a durable job.
        Some(Lane::Translation) => Some("translation"),
        Some(Lane::Digest) => Some("digest"),
        Some(Lane::Research) => Some("research"),
        None => None,
    };
    let mut conn = store.conn()?;
    // Serialize selection + lease acquisition even when separate processes use this DB.
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let at = chrono::Utc::now().timestamp();
    let row = tx
        .query_row(
            "WITH lanes AS (
            SELECT *,CASE kind WHEN 'material_translation' THEN 'translation'
                WHEN 'digest' THEN 'digest' ELSE 'research' END AS lane FROM jobs
        )
        SELECT j.id,j.user_id,j.kind,j.payload,j.attempt,j.lane FROM lanes j
        WHERE (?2 IS NULL OR j.lane=?2) AND j.attempt<3
            AND ((j.status='queued' AND j.next_at<=?1)
                OR (j.status='running' AND j.lease_until<?1))
            AND NOT EXISTS(SELECT 1 FROM lanes active
                WHERE active.lane=j.lane AND active.status='running' AND active.lease_until>=?1
                    AND (?2 IS NOT NULL OR active.user_id=j.user_id))
        ORDER BY CASE WHEN ?2 IS NULL
            THEN COALESCE((SELECT last_turn FROM scheduler_turns WHERE user_id=j.user_id),0)
            ELSE COALESCE((SELECT last_turn FROM scheduler_lane_turns
                WHERE user_id=j.user_id AND lane=j.lane),0) END,
            CASE WHEN j.status='running' THEN 0 ELSE 1 END,
            CASE WHEN j.kind='digest' AND (json_extract(j.payload,'$.researchDate') IS NOT NULL
                OR json_extract(j.payload,'$.deliverImmediately')=1) THEN 0 ELSE 1 END,
            CASE WHEN j.kind='digest' THEN COALESCE(json_extract(j.payload,'$.availableAt'),
                json_extract(j.payload,'$.researchDate'),'') ELSE '' END DESC,
            j.created,j.id LIMIT 1",
            params![at, lane],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, String>(5)?,
                ))
            },
        )
        .optional()?;
    let Some((id, user, kind, payload, attempt, lane)) = row else {
        return Ok(None);
    };
    let payload = serde_json::from_str(&payload)?;
    tx.execute(
        "UPDATE jobs SET status='running',attempt=attempt+1,lease_until=?,message='正在处理' WHERE id=?",
        params![at + 960, id],
    )?;
    tx.execute(
        "INSERT INTO scheduler_lane_turns(user_id,lane,last_turn)
            VALUES(?1,?2,(SELECT COALESCE(MAX(last_turn),0)+1 FROM scheduler_lane_turns WHERE lane=?2))
            ON CONFLICT(user_id,lane) DO UPDATE SET last_turn=excluded.last_turn",
        params![user, lane],
    )?;
    tx.execute(
        "INSERT INTO scheduler_turns(user_id,last_turn)
            VALUES(?,(SELECT COALESCE(MAX(last_turn),0)+1 FROM scheduler_turns))
            ON CONFLICT(user_id) DO UPDATE SET last_turn=excluded.last_turn",
        [&user],
    )?;
    tx.commit()?;
    Ok(Some(Job {
        id,
        user,
        kind,
        payload,
        attempt: attempt + 1,
    }))
}
