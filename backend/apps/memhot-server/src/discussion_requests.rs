//! Request-level replay protects the whole search/model/conversation operation.
//! A pending request never repeats paid work after a timeout or process restart.
use crate::{
    error::{Error, Result},
    hash, now, App,
};
use axum::http::{HeaderMap, StatusCode};
use rusqlite::{params, OptionalExtension, Transaction};
use serde_json::{json, Value};

pub fn key(headers: &HeaderMap) -> Result<String> {
    // /v1 historically accepted requests without this header. Updated clients always send it.
    if headers.contains_key("idempotency-key") {
        crate::billing::api::request_key(headers)
    } else {
        Ok(crate::id())
    }
}
pub fn begin(app: &App, user: &str, key: &str, input: &Value) -> Result<Option<Value>> {
    let fingerprint = hash(input.to_string().as_bytes());
    let mut conn = app.store.conn()?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let old: Option<String> = tx
        .query_row(
            "SELECT data FROM records WHERE user_id=? AND kind='discussion_request' AND id=?",
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
                "同一个请求标识不能用于不同的讨论",
            ));
        }
        if old["status"] == "completed" {
            return Ok(Some(old["response"].clone()));
        }
        if old["status"] == "failed" {
            return Err(Error::new(
                StatusCode::CONFLICT,
                "request_failed",
                "上次讨论未能完成。你可以再次发送；系统不会自动重复扣费重试。",
            ));
        }
        return Err(Error::new(
            StatusCode::CONFLICT,
            "outcome_unknown",
            "这次讨论已提交但尚无可读取结果。请刷新历史；如需重新请求，请明确选择再次发送。",
        ));
    }
    let data = json!({"id":key,"fingerprint":fingerprint,"status":"pending","createdAt":now()});
    tx.execute(
        "INSERT INTO records(user_id,kind,id,data,created) VALUES(?,'discussion_request',?,?,?)",
        params![user, key, data.to_string(), data["createdAt"].as_str()],
    )?;
    tx.commit()?;
    Ok(None)
}
pub fn complete(tx: &Transaction<'_>, user: &str, key: &str, response: &Value) -> Result<()> {
    let updated = tx.execute("UPDATE records SET data=json_set(data,'$.status','completed','$.response',json(?)) WHERE user_id=? AND kind='discussion_request' AND id=? AND json_extract(data,'$.status')='pending'",params![response.to_string(),user,key])?;
    if updated != 1 {
        return Err(Error::missing());
    }
    Ok(())
}
pub fn failed(app: &App, user: &str, key: &str) -> Result<()> {
    app.store.conn()?.execute("UPDATE records SET data=json_set(data,'$.status','failed') WHERE user_id=? AND kind='discussion_request' AND id=? AND json_extract(data,'$.status')='pending'",params![user,key])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pending_does_not_repeat_and_completed_is_tenant_scoped() {
        let dir = std::env::temp_dir().join(format!("memhot-discuss-request-{}", crate::id()));
        let app = App::open(dir.clone()).unwrap();
        let (a, _) = crate::auth::create_pair(&app, "A", None).unwrap();
        let (b, _) = crate::auth::create_pair(&app, "B", None).unwrap();
        let input = json!({"message":"example"});
        assert!(begin(&app, &a, "key", &input).unwrap().is_none());
        assert_eq!(
            begin(&app, &a, "key", &input).unwrap_err().code,
            "outcome_unknown"
        );
        assert!(begin(&app, &b, "key", &input).unwrap().is_none());
        assert_eq!(
            begin(&app, &a, "key", &json!({"message":"different"}))
                .unwrap_err()
                .code,
            "idempotency_conflict"
        );
        {
            let mut conn = app.store.conn().unwrap();
            let tx = conn.transaction().unwrap();
            complete(&tx, &a, "key", &json!({"answer":"once"})).unwrap();
            tx.commit().unwrap();
        }
        assert_eq!(
            begin(&app, &a, "key", &input).unwrap(),
            Some(json!({"answer":"once"}))
        );
        assert_eq!(
            begin(&app, &b, "key", &input).unwrap_err().code,
            "outcome_unknown"
        );
        failed(&app, &b, "key").unwrap();
        assert_eq!(
            begin(&app, &b, "key", &input).unwrap_err().code,
            "request_failed"
        );
        // A repeated failed request remains inert. Only an explicit new request can execute.
        assert_eq!(
            begin(&app, &b, "key", &input).unwrap_err().code,
            "request_failed"
        );
        assert!(begin(&app, &b, "new-request", &input).unwrap().is_none());
        let _ = std::fs::remove_dir_all(dir);
    }
}
