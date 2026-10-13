use super::*;
use crate::auth::User;
use axum::{extract::State, http::HeaderMap, Json};
use serde::Deserialize;
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Topup {
    package_id: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Plan {
    plan_id: String,
}
pub fn request_key(headers: &HeaderMap) -> Result<String> {
    headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .filter(|v| !v.is_empty() && v.len() <= 200 && v.is_ascii())
        .map(str::to_owned)
        .ok_or_else(|| Error::bad("缺少有效Idempotency-Key"))
}
fn replay(
    tx: &Transaction<'_>,
    user: &str,
    key: &str,
    fingerprint: &str,
) -> Result<Option<String>> {
    let row: Option<(String, String)> = tx
        .query_row(
            "SELECT fingerprint,event_id FROM billing_orders WHERE user_id=? AND request_key=?",
            params![user, key],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    row.map(|(old, eid)| {
        if old == fingerprint {
            Ok(eid)
        } else {
            Err(Error::new(
                StatusCode::CONFLICT,
                "idempotency_conflict",
                "相同请求ID已用于不同测试订单",
            ))
        }
    })
    .transpose()
}
fn order(tx: &Transaction<'_>, user: &str, key: &str, fingerprint: &str, eid: &str) -> Result<()> {
    tx.execute("INSERT INTO billing_orders(user_id,request_key,fingerprint,event_id,created) VALUES(?,?,?,?,?)",params![user,key,fingerprint,eid,chrono::Utc::now().timestamp()])?;
    Ok(())
}
pub fn purchase(app: &App, user: &str, key: &str, package: &str) -> Result<String> {
    let credits = match package {
        "test_100" => 100,
        "test_500" => 500,
        _ => return Err(Error::bad("测试充值包不存在")),
    };
    let fingerprint = format!("topup:{package}");
    let mut conn = app.store.conn()?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    if let Some(eid) = replay(&tx, user, key, &fingerprint)? {
        return Ok(eid);
    }
    ensure_wallet(&tx, user)?;
    expire(&tx, user)?;
    let recent:i64=tx.query_row("SELECT count(*) FROM billing_orders WHERE user_id=? AND fingerprint LIKE 'topup:%' AND created>?",params![user,chrono::Utc::now().timestamp()-86400],|r|r.get(0))?;
    let balance: i64 = tx.query_row(
        "SELECT included+topup+reserved FROM billing_wallets WHERE user_id=?",
        [user],
        |r| r.get(0),
    )?;
    if recent >= 5 || balance + credits > 5000 {
        return Err(Error::new(
            StatusCode::TOO_MANY_REQUESTS,
            "test_topup_limit",
            "测试充值每天最多5次，余额上限5000测试点",
        ));
    }
    tx.execute(
        "UPDATE billing_wallets SET topup=topup+?,granted=granted+? WHERE user_id=?",
        params![credits, credits, user],
    )?;
    let eid = event(
        &tx,
        user,
        &format!("order:{key}"),
        "test_topup",
        &format!("测试充值 · {credits}测试点（无需付款）"),
        credits,
    )?;
    order(&tx, user, key, &fingerprint, &eid)?;
    tx.commit()?;
    Ok(eid)
}
pub fn subscribe(app: &App, user: &str, key: &str, plan: &str) -> Result<String> {
    let (credits, days, label) = match plan {
        "trial" => (100, 7, "邀请试用"),
        "beta" => (300, 30, "探索测试套餐"),
        "plus" => (1000, 30, "深入测试套餐"),
        _ => return Err(Error::bad("测试套餐不存在")),
    };
    let fingerprint = format!("plan:{plan}");
    let at = chrono::Utc::now().timestamp();
    let mut conn = app.store.conn()?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    if let Some(eid) = replay(&tx, user, key, &fingerprint)? {
        return Ok(eid);
    }
    ensure_wallet(&tx, user)?;
    expire(&tx, user)?;
    let (current, cycle_start, expires, granted): (String, i64, i64, i64) = tx.query_row(
        "SELECT plan_id,cycle_start,expires,cycle_granted FROM billing_wallets WHERE user_id=?",
        [user],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )?;
    if current == plan && expires > at {
        return Err(Error::new(
            StatusCode::CONFLICT,
            "plan_already_active",
            "当前测试套餐已经生效",
        ));
    }
    if plan == "trial" {
        return Err(Error::new(
            StatusCode::CONFLICT,
            "trial_already_granted",
            "邀请试用仅领取一次，请选择测试套餐",
        ));
    }
    let (delta, start, end, cycle) = if expires > at {
        if credits < granted {
            return Err(Error::new(
                StatusCode::CONFLICT,
                "plan_downgrade_unavailable",
                "当前周期请保留现有套餐，到期后再切换",
            ));
        }
        (
            credits - granted,
            cycle_start,
            expires.max(cycle_start + days * 86400),
            None,
        )
    } else {
        (credits, at, at + days * 86400, Some(id()))
    };
    if let Some(cycle) = cycle {
        tx.execute("UPDATE billing_wallets SET plan_id=?,cycle_id=?,cycle_start=?,expires=?,included=?,cycle_granted=?,granted=granted+? WHERE user_id=?",params![plan,cycle,start,end,delta,credits,delta,user])?;
    } else {
        tx.execute("UPDATE billing_wallets SET plan_id=?,expires=?,included=included+?,cycle_granted=?,granted=granted+? WHERE user_id=?",params![plan,end,delta,credits,delta,user])?;
    }
    let eid = event(
        &tx,
        user,
        &format!("order:{key}"),
        "test_plan",
        &format!("{label} · 测试激活（无需付款）"),
        delta,
    )?;
    order(&tx, user, key, &fingerprint, &eid)?;
    tx.commit()?;
    Ok(eid)
}
fn response(app: &App, user: &str, eid: &str) -> Result<Json<Value>> {
    resume_blocked(app, user)?;
    let transaction = event_json(&*app.store.conn()?, user, eid)?;
    Ok(Json(
        json!({"transaction":transaction,"account":account(app,user)?}),
    ))
}
pub async fn topup(
    State(app): State<App>,
    User(user): User,
    headers: HeaderMap,
    Json(input): Json<Topup>,
) -> Result<Json<Value>> {
    let key = request_key(&headers)?;
    let eid = purchase(&app, &user, &key, &input.package_id)?;
    response(&app, &user, &eid)
}
pub async fn plan(
    State(app): State<App>,
    User(user): User,
    headers: HeaderMap,
    Json(input): Json<Plan>,
) -> Result<Json<Value>> {
    let key = request_key(&headers)?;
    let eid = subscribe(&app, &user, &key, &input.plan_id)?;
    response(&app, &user, &eid)
}
