use crate::{
    auth::User,
    error::{Error, Result},
    hash, id, now, App,
};
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    Json,
};
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};

/// Explicit refresh is an idempotent scheduling operation; it does not depend on push delivery.
pub async fn queue(
    State(app): State<App>,
    User(user): User,
    headers: HeaderMap,
    Json(input): Json<Value>,
) -> Result<(StatusCode, Json<Value>)> {
    crate::accounts::interaction_rate(&app, &user)?;
    let client = crate::discussion_requests::key(&headers)?;
    let key = hash(client.as_bytes());
    let fingerprint = hash(input.to_string().as_bytes());
    let mut conn = app.store.conn()?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let old: Option<String> = tx
        .query_row(
            "SELECT data FROM records WHERE user_id=? AND kind='research_request' AND id=?",
            params![user, key],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(old) = old {
        let old: Value = serde_json::from_str(&old)?;
        if old["fingerprint"] != fingerprint {
            return Err(Error::new(
                StatusCode::CONFLICT,
                "idempotency_conflict",
                "相同请求ID不能用于不同的更新操作",
            ));
        }
        return Ok((StatusCode::ACCEPTED, Json(old["response"].clone())));
    }
    if input.get("kind").is_some()
        && !input["kind"].is_null()
        && !matches!(input["kind"].as_str(), Some("research" | "digest"))
    {
        return Err(Error::bad("更新类型无效"));
    }
    let mut targets = Vec::new();
    if let Some(thought) = input["thoughtId"].as_str() {
        let raw: String = tx
            .query_row(
                "SELECT data FROM records WHERE user_id=? AND kind='thought' AND id=?",
                params![user, thought],
                |r| r.get(0),
            )
            .optional()?
            .ok_or_else(Error::missing)?;
        let value: Value = serde_json::from_str(&raw)?;
        if !matches!(value["status"].as_str(), Some("open" | "kept")) {
            return Err(Error::bad("这个思考已放下或删除，请先重新启用"));
        }
        targets.push(("research", json!({"thoughtId":thought,"kind":"research"})));
    } else if input["kind"] != "digest" {
        let mut statement=tx.prepare("SELECT id FROM records WHERE user_id=? AND kind='thought' AND json_extract(data,'$.status') IN ('open','kept') AND coalesce(json_extract(data,'$.researchEligible'),1)=1 ORDER BY created,id")?;
        let ids = statement
            .query_map([&user], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        for thought in ids {
            targets.push(("research", json!({"thoughtId":thought,"kind":"research"})));
        }
    }
    if input["thoughtId"].is_null() && input["kind"] != "research" {
        targets.push(("digest", json!({"kind":"digest"})));
    }
    if targets.is_empty() {
        return Err(Error::bad(
            "还没有可研究的问题，请先录音、导入对话或补充一个思考",
        ));
    }
    let mut jobs = Vec::new();
    for (kind, mut payload) in targets {
        let pending: Vec<(String, String)> = {
            let mut stmt = tx.prepare("SELECT id,status FROM jobs WHERE user_id=?1 AND kind=?2 AND status IN ('queued','running','blocked') AND json_extract(payload,'$.thoughtId') IS ?3 ORDER BY created,id")?;
            let rows = stmt.query_map(params![user, kind, payload["thoughtId"].as_str()], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })?;
            rows.collect::<std::result::Result<_, _>>()?
        };
        if !pending.is_empty() {
            for (key, status) in pending {
                // The worker may already hold its old scheduled payload; finish reads this flag
                // under the same transaction as publication, so no duplicate paid run is needed.
                tx.execute("UPDATE jobs SET payload=json_set(json_remove(payload,'$.availableAt'),'$.deliverImmediately',json('true')) WHERE id=? AND user_id=?", params![key,user])?;
                jobs.push(json!({"id":key,"status":status,"kind":kind}));
            }
            continue;
        }
        payload["deliverImmediately"] = json!(true);
        let job = id();
        tx.execute("INSERT INTO jobs(id,user_id,kind,payload,status,created,dedup) VALUES(?,?,?,?,'queued',?,?)",params![job,user,kind,payload.to_string(),now(),format!("manual:{key}:{kind}:{}",payload["thoughtId"])])?;
        jobs.push(json!({"id":job,"status":"queued","kind":kind}));
    }
    let response = json!({"id":jobs[0]["id"],"status":"queued","jobs":jobs});
    let receipt = json!({"id":key,"fingerprint":fingerprint,"response":response,"createdAt":now()});
    tx.execute(
        "INSERT INTO records(user_id,kind,id,data,created) VALUES(?,'research_request',?,?,?)",
        params![user, key, receipt.to_string(), now()],
    )?;
    tx.commit()?;
    Ok((StatusCode::ACCEPTED, Json(response)))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn manual_refresh_all_and_replay_avoids_hidden_three_limit() {
        let dir = std::env::temp_dir().join(format!("memhot-refresh-{}", id()));
        let app = App::open(dir.clone()).unwrap();
        let (user, _) = crate::auth::create_pair(&app, "A", None).unwrap();
        for n in 0..65 {
            app.store.put(&user,"thought",&json!({"id":format!("t{n}"),"status":"open","createdAt":now(),"researchEligible":true}),None).unwrap();
        }
        let mut headers = HeaderMap::new();
        headers.insert("idempotency-key", "refresh".parse().unwrap());
        let input = json!({"kind":"research"});
        let (_, Json(first)) = queue(
            State(app.clone()),
            User(user.clone()),
            headers.clone(),
            Json(input.clone()),
        )
        .await
        .unwrap();
        assert_eq!(first["jobs"].as_array().unwrap().len(), 65);
        let (_, Json(second)) = queue(
            State(app.clone()),
            User(user.clone()),
            headers.clone(),
            Json(input),
        )
        .await
        .unwrap();
        assert_eq!(first, second);
        let mut another = HeaderMap::new();
        another.insert("idempotency-key", "another-refresh".parse().unwrap());
        let (_, Json(again)) = queue(
            State(app.clone()),
            User(user.clone()),
            another,
            Json(json!({"kind":"research"})),
        )
        .await
        .unwrap();
        assert_eq!(first["jobs"], again["jobs"]);
        assert_eq!(
            queue(
                State(app.clone()),
                User(user),
                headers,
                Json(json!({"kind":"digest"}))
            )
            .await
            .unwrap_err()
            .code,
            "idempotency_conflict"
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
