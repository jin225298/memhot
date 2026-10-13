//! Test credits only. No payment processor, currency, or real-money purchase exists.
use crate::{
    error::{Error, Result},
    id, now, App,
};
use axum::http::StatusCode;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde_json::{json, Value};
#[path = "billing_api.rs"]
pub mod api;
pub use api::{plan, topup};
pub fn migrate(conn: &Connection) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    conn.execute_batch("CREATE TABLE IF NOT EXISTS billing_wallets(user_id TEXT PRIMARY KEY REFERENCES users(id),plan_id TEXT NOT NULL,cycle_id TEXT NOT NULL,cycle_start INTEGER NOT NULL,expires INTEGER NOT NULL,included INTEGER NOT NULL CHECK(included>=0),topup INTEGER NOT NULL DEFAULT 0 CHECK(topup>=0),reserved INTEGER NOT NULL DEFAULT 0 CHECK(reserved>=0),spent INTEGER NOT NULL DEFAULT 0,granted INTEGER NOT NULL,cycle_granted INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS billing_events(id TEXT PRIMARY KEY,user_id TEXT NOT NULL REFERENCES users(id),event_key TEXT NOT NULL,kind TEXT NOT NULL,label TEXT NOT NULL,credits INTEGER NOT NULL,created TEXT NOT NULL,UNIQUE(user_id,event_key));
CREATE TABLE IF NOT EXISTS billing_usage(user_id TEXT NOT NULL REFERENCES users(id),operation_key TEXT NOT NULL,fingerprint TEXT NOT NULL,kind TEXT NOT NULL,units INTEGER NOT NULL,state TEXT NOT NULL,included INTEGER NOT NULL,topup INTEGER NOT NULL,cycle_id TEXT NOT NULL,attempt INTEGER NOT NULL DEFAULT 1,created INTEGER NOT NULL,PRIMARY KEY(user_id,operation_key));
CREATE TABLE IF NOT EXISTS billing_orders(user_id TEXT NOT NULL REFERENCES users(id),request_key TEXT NOT NULL,fingerprint TEXT NOT NULL,event_id TEXT NOT NULL,created INTEGER NOT NULL,PRIMARY KEY(user_id,request_key));")?;
    let users: Vec<String> = {
        let mut stmt = conn.prepare("SELECT id FROM users")?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        rows.collect::<std::result::Result<_, _>>()?
    };
    for user in users {
        ensure_wallet(&tx, &user)?;
    }
    tx.commit()?;
    Ok(())
}
pub fn ensure_wallet(conn: &Connection, user: &str) -> Result<()> {
    let at = chrono::Utc::now().timestamp();
    let changed=conn.execute("INSERT OR IGNORE INTO billing_wallets(user_id,plan_id,cycle_id,cycle_start,expires,included,granted,cycle_granted) VALUES(?,'trial',?,?,?,100,100,100)",params![user,id(),at,at+7*86400])?;
    if changed > 0 {
        event(
            conn,
            user,
            "initial-trial",
            "grant",
            "邀请试用 · 100测试点",
            100,
        )?;
    }
    Ok(())
}
pub fn event(
    conn: &Connection,
    user: &str,
    key: &str,
    kind: &str,
    label: &str,
    credits: i64,
) -> Result<String> {
    let eid = id();
    conn.execute("INSERT INTO billing_events(id,user_id,event_key,kind,label,credits,created) VALUES(?,?,?,?,?,?,?)",params![eid,user,key,kind,label,credits,now()])?;
    Ok(eid)
}
pub fn expire(conn: &Connection, user: &str) -> Result<()> {
    let row:Option<(String,i64)>=conn.query_row("SELECT cycle_id,included FROM billing_wallets WHERE user_id=? AND expires<=? AND included>0",params![user,chrono::Utc::now().timestamp()],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    if let Some((cycle, remaining)) = row {
        conn.execute(
            "UPDATE billing_wallets SET included=0 WHERE user_id=?",
            [user],
        )?;
        event(
            conn,
            user,
            &format!("expire:{cycle}"),
            "expired",
            "套餐到期，未使用赠点失效",
            -remaining,
        )?;
    }
    Ok(())
}
pub fn exhausted() -> Error {
    Error::new(
        StatusCode::PAYMENT_REQUIRED,
        "quota_exhausted",
        "测试额度不足，请在账号中领取测试套餐或测试充值",
    )
}
pub fn ensure_available(app: &App, user: &str, units: i64) -> Result<()> {
    if units < 1 || units > 100 {
        return Err(Error::bad("额度请求无效"));
    }
    let mut conn = app.store.conn()?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    ensure_wallet(&tx, user)?;
    expire(&tx, user)?;
    let available: i64 = tx.query_row(
        "SELECT included+topup FROM billing_wallets WHERE user_id=?",
        [user],
        |r| r.get(0),
    )?;
    tx.commit()?;
    if available < units {
        return Err(exhausted());
    }
    Ok(())
}
/// Shared public editions require an active test plan, not a positive point balance.
pub fn ensure_subscription(app: &App, user: &str) -> Result<()> {
    let mut conn = app.store.conn()?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let enabled: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM users u LEFT JOIN accounts a ON a.user_id=u.id WHERE u.id=? AND COALESCE(a.disabled,0)=0)",
        [user],
        |r| r.get(0),
    )?;
    let eligible = if enabled {
        ensure_wallet(&tx, user)?;
        tx.query_row(
            "SELECT expires>? FROM billing_wallets WHERE user_id=?",
            params![chrono::Utc::now().timestamp(), user],
            |r| r.get::<_, bool>(0),
        )?
    } else {
        false
    };
    tx.commit()?;
    if !eligible {
        return Err(Error::new(
            StatusCode::PAYMENT_REQUIRED,
            "subscription_required",
            "订阅已到期，请在账号中领取测试套餐后继续同步公共日报",
        ));
    }
    Ok(())
}
pub fn reserve_tx(
    tx: &Transaction<'_>,
    user: &str,
    key: &str,
    fingerprint: &str,
    kind: &str,
    units: i64,
) -> Result<()> {
    if units < 1 || units > 100 || !matches!(kind, "text" | "asr") {
        return Err(Error::bad("额度请求无效"));
    }
    ensure_wallet(tx, user)?;
    expire(tx, user)?;
    let old:Option<(String,String,i64,String,i64)>=tx.query_row("SELECT state,fingerprint,units,kind,attempt FROM billing_usage WHERE user_id=? AND operation_key=?",params![user,key],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional()?;
    let attempt = if let Some((state, old_hash, old_units, old_kind, attempt)) = old {
        if old_hash != fingerprint || old_units != units || old_kind != kind {
            return Err(Error::new(
                StatusCode::CONFLICT,
                "idempotency_conflict",
                "相同请求ID已用于不同内容",
            ));
        }
        if state != "released" {
            return Err(Error::new(
                StatusCode::CONFLICT,
                "outcome_unknown",
                "此请求已执行或结果待确认，已阻止重复消耗额度",
            ));
        }
        attempt + 1
    } else {
        1
    };
    let (included, topup, cycle): (i64, i64, String) = tx.query_row(
        "SELECT included,topup,cycle_id FROM billing_wallets WHERE user_id=?",
        [user],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    if included + topup < units {
        return Err(exhausted());
    }
    let from_included = units.min(included);
    let from_topup = units - from_included;
    tx.execute("UPDATE billing_wallets SET included=included-?,topup=topup-?,reserved=reserved+? WHERE user_id=?",params![from_included,from_topup,units,user])?;
    tx.execute("INSERT INTO billing_usage(user_id,operation_key,fingerprint,kind,units,state,included,topup,cycle_id,attempt,created) VALUES(?,?,?,?,?,'reserved',?,?,?,?,?) ON CONFLICT(user_id,operation_key) DO UPDATE SET state='reserved',included=excluded.included,topup=excluded.topup,cycle_id=excluded.cycle_id,attempt=excluded.attempt,created=excluded.created",params![user,key,fingerprint,kind,units,from_included,from_topup,cycle,attempt,chrono::Utc::now().timestamp()])?;
    event(
        tx,
        user,
        &format!("usage:{key}:reserve:{attempt}"),
        "reserved",
        if kind == "asr" {
            "转写额度预占"
        } else {
            "AI调用额度预占"
        },
        -units,
    )?;
    Ok(())
}
pub fn settle_tx(tx: &Transaction<'_>, user: &str, key: &str, outcome: &str) -> Result<()> {
    let row:Option<(i64,i64,i64,String,String,i64)>=tx.query_row("SELECT units,included,topup,cycle_id,state,attempt FROM billing_usage WHERE user_id=? AND operation_key=?",params![user,key],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).optional()?;
    let Some((units, included, topup, cycle, state, attempt)) = row else {
        return Err(Error::missing());
    };
    if !matches!(state.as_str(), "reserved" | "unknown") {
        return Ok(());
    }
    if outcome == "unknown" {
        tx.execute(
            "UPDATE billing_usage SET state='unknown' WHERE user_id=? AND operation_key=?",
            params![user, key],
        )?;
        return Ok(());
    }
    if !matches!(outcome, "spent" | "released") {
        return Err(Error::internal());
    }
    let refund_included = if outcome == "released" {
        let eligible: bool = tx.query_row(
            "SELECT cycle_id=? AND expires>? FROM billing_wallets WHERE user_id=?",
            params![cycle, chrono::Utc::now().timestamp(), user],
            |r| r.get(0),
        )?;
        if eligible {
            included
        } else {
            0
        }
    } else {
        0
    };
    let refund_topup = if outcome == "released" { topup } else { 0 };
    tx.execute("UPDATE billing_wallets SET reserved=reserved-?,included=included+?,topup=topup+?,spent=spent+? WHERE user_id=? AND reserved>=?",params![units,refund_included,refund_topup,if outcome=="spent"{units}else{0},user,units])?;
    tx.execute(
        "UPDATE billing_usage SET state=? WHERE user_id=? AND operation_key=?",
        params![outcome, user, key],
    )?;
    event(
        tx,
        user,
        &format!("usage:{key}:{outcome}:{attempt}"),
        outcome,
        if outcome == "spent" {
            "AI调用已结算"
        } else {
            "请求未完成，额度已返还"
        },
        refund_included + refund_topup,
    )?;
    Ok(())
}
pub fn plans() -> Value {
    json!([
        {"id":"trial","name":"邀请试用","credits":100,"durationDays":7},
        {"id":"beta","name":"探索测试套餐","credits":300,"durationDays":30},
        {"id":"plus","name":"深入测试套餐","credits":1000,"durationDays":30}
    ])
}
pub fn packages() -> Value {
    json!([
        {"id":"test_100","name":"100测试点","credits":100},
        {"id":"test_500","name":"500测试点","credits":500}
    ])
}
pub fn event_json(conn: &Connection, user: &str, eid: &str) -> Result<Value> {
    conn.query_row("SELECT id,kind,label,credits,created FROM billing_events WHERE user_id=? AND id=?",params![user,eid],|r|Ok(json!({"id":r.get::<_,String>(0)?,"kind":r.get::<_,String>(1)?,"label":r.get::<_,String>(2)?,"credits":r.get::<_,i64>(3)?,"createdAt":r.get::<_,String>(4)?,"mode":"test"}))).optional()?.ok_or_else(Error::missing)
}
pub fn account(app: &App, user: &str) -> Result<Value> {
    let mut conn = app.store.conn()?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    ensure_wallet(&tx, user)?;
    expire(&tx, user)?;
    let mut account=tx.query_row("SELECT plan_id,expires,included,topup,reserved,spent,granted FROM billing_wallets WHERE user_id=?",[user],|r|{
        let plan:String=r.get(0)?;let expires:i64=r.get(1)?;let included:i64=r.get(2)?;let topup:i64=r.get(3)?;
        Ok(json!({"mode":"test","subscription":{"planId":plan,"label":match plan.as_str(){"beta"=>"探索测试套餐","plus"=>"深入测试套餐",_=>"邀请试用"},"expiresAt":expires,"active":expires>chrono::Utc::now().timestamp()},"quota":{"available":included+topup,"included":included,"topup":topup,"reserved":r.get::<_,i64>(4)?,"used":r.get::<_,i64>(5)?,"totalGranted":r.get::<_,i64>(6)?,"unit":"测试点"}}))
    })?;
    account["user"] = crate::accounts::user_json(&tx, user)?;
    account["plans"] = plans();
    account["topupPackages"] = packages();
    account["rules"] = json!([
        "每次文本模型调用消耗1测试点",
        "云转写每30秒消耗1测试点，不足30秒按30秒计",
        "所有套餐与充值均为测试，不收取真实费用",
        "套餐赠点到期失效；测试充值点在本次邀请测试期间有效",
        "请求结果不明时暂时保留额度预占，避免重复调用"
    ]);
    account["passwordReset"] = json!({"mode":"admin_code","message":"邀请测试期间请联系管理员获取重置码；系统尚未发送找回邮件"});
    let ids: Vec<String> = {
        let mut stmt = tx.prepare(
            "SELECT id FROM billing_events WHERE user_id=? ORDER BY rowid DESC LIMIT 30",
        )?;
        let rows = stmt.query_map([user], |r| r.get(0))?;
        rows.collect::<std::result::Result<_, _>>()?
    };
    account["transactions"] = json!(ids
        .into_iter()
        .map(|eid| event_json(&tx, user, &eid))
        .collect::<Result<Vec<_>>>()?);
    tx.commit()?;
    Ok(account)
}
pub fn resume_blocked(app: &App, user: &str) -> Result<()> {
    app.store.conn()?.execute("UPDATE jobs SET status='queued',attempt=0,next_at=0,message='测试额度已补充，等待继续' WHERE user_id=? AND status='blocked'",[user])?;
    Ok(())
}
