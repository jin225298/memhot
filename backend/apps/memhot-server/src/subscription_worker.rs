//! Independent, bounded collection loop: no model gate, research queue, or paid calls.
use crate::{
    error::{Error, Result},
    subscription_items, App,
};
use rusqlite::{params, OptionalExtension};
use std::time::Duration;

pub const LEASE_SECONDS: i64 = 90;
pub const POLL_SECONDS: i64 = 900;
#[derive(Clone)]
pub struct Claim {
    pub id: String,
    pub url: String,
    pub token: String,
    pub failures: i64,
}

pub async fn run(app: App) {
    let mut tick = tokio::time::interval(Duration::from_secs(2));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut maintenance = tokio::time::interval(Duration::from_secs(60));
    let mut running = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            _=tick.tick()=>{},
            _=maintenance.tick()=>{if let Err(e)=subscription_items::retention(&app,chrono::Utc::now().timestamp()){eprintln!("subscription retention code={}",e.code);}},
            _=running.join_next(),if !running.is_empty()=>{},
        }
        while running.len() < 4 {
            let claim = match claim(&app, chrono::Utc::now().timestamp()) {
                Ok(Some(c)) => c,
                Ok(None) => break,
                Err(e) => {
                    eprintln!("subscription claim code={}", e.code);
                    break;
                }
            };
            let app = app.clone();
            running.spawn(async move {
                let result = tokio::time::timeout(Duration::from_secs(30), collect(&claim))
                    .await
                    .unwrap_or_else(|_| Err(Error::upstream("timeout", "订阅来源响应超时")));
                if let Err(e) = finish(&app, &claim, result, chrono::Utc::now().timestamp()) {
                    eprintln!("subscription finish code={}", e.code);
                }
            });
        }
    }
}
async fn collect(claim: &Claim) -> Result<Vec<crate::discovery::Material>> {
    let data = crate::network::fetch(&claim.url).await?;
    crate::subscription_preview::parse_feed(&data.text, &data.url)
        .map_err(|_| Error::upstream("invalid_feed", "来源未返回完整有效的RSS或Atom订阅"))
}
pub fn claim(app: &App, at: i64) -> Result<Option<Claim>> {
    let mut conn = app.store.conn()?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let row=tx.query_row("SELECT s.id,s.url,s.failures FROM subscription_sources s WHERE s.next_at<=?1 AND s.lease_until<=?1 AND (SELECT count(*) FROM subscription_sources active WHERE active.lease_until>?1)<4 AND NOT EXISTS(SELECT 1 FROM subscription_sources active WHERE active.host=s.host AND active.lease_until>?1) AND EXISTS(SELECT 1 FROM subscriptions u WHERE u.source_id=s.id AND u.enabled=1) ORDER BY s.next_at,s.id LIMIT 1",[at],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,i64>(2)?))).optional()?;
    let Some((id, url, failures)) = row else {
        return Ok(None);
    };
    let token = crate::id();
    tx.execute(
        "UPDATE subscription_sources SET lease_token=?,lease_until=?,last_attempt=? WHERE id=?",
        params![token, at + LEASE_SECONDS, at, id],
    )?;
    tx.commit()?;
    Ok(Some(Claim {
        id,
        url,
        token,
        failures,
    }))
}
pub fn finish(
    app: &App,
    claim: &Claim,
    result: Result<Vec<crate::discovery::Material>>,
    at: i64,
) -> Result<bool> {
    let mut conn = app.store.conn()?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let current:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM subscription_sources s WHERE s.id=?1 AND s.lease_token=?2 AND s.lease_until>?3 AND EXISTS(SELECT 1 FROM subscriptions u WHERE u.source_id=s.id AND u.enabled=1))",params![claim.id,claim.token,at],|r|r.get(0))?;
    if !current {
        return Ok(false);
    }
    match result {
        Ok(items) => {
            let count = subscription_items::persist(&tx, &claim.id, &items, at)?;
            tx.execute("UPDATE subscription_sources SET lease_token=NULL,lease_until=0,last_success=?,next_at=?,failures=0,last_new=?,error=NULL WHERE id=? AND lease_token=?",params![at,at+POLL_SECONDS,count,claim.id,claim.token])?;
        }
        Err(e) => {
            let failures = (claim.failures + 1).min(8);
            let delay = (60_i64 * (1_i64 << failures)).min(6 * 3600);
            // External error strings can include signed URLs; only a fixed local vocabulary is stored.
            let message = match e.code {
                "timeout" => "来源响应超时，稍后自动重试",
                "network" => "暂时无法连接来源，稍后自动重试",
                "rate_limited" => "来源暂时限流，稍后自动重试",
                "invalid_feed" => "来源未返回完整有效的RSS或Atom订阅",
                "response_too_large" => "来源内容超过大小限制",
                "provider_auth" => "来源拒绝访问，请检查订阅地址",
                _ => "订阅读取失败，稍后自动重试",
            };
            tx.execute("UPDATE subscription_sources SET lease_token=NULL,lease_until=0,next_at=?,failures=?,error=? WHERE id=? AND lease_token=?",params![at+delay,failures,message,claim.id,claim.token])?;
        }
    }
    tx.commit()?;
    Ok(true)
}
