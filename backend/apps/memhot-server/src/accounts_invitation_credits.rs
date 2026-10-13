//! Operator-only invitation allotments. Redemption shares the registration transaction.
use crate::{
    accounts::normalize_email,
    billing,
    error::{Error, Result},
    App,
};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};

pub fn migrate(conn: &Connection) -> Result<()> {
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('account_invites') WHERE name='test_credits')",
        [], |r| r.get(0),
    )?;
    if !exists {
        conn.execute_batch("ALTER TABLE account_invites ADD COLUMN test_credits INTEGER NOT NULL DEFAULT 100 CHECK(test_credits BETWEEN 100 AND 5000)")?;
    }
    Ok(())
}

/// Returns true when the allotment will be granted at registration, false when
/// applied to an already registered trial. Serialize with registration so neither
/// ordering can lose the operator's change or grant it twice.
pub fn set(app: &App, email: &str, credits: i64) -> Result<bool> {
    let email = normalize_email(email)?;
    if !(100..=5000).contains(&credits) {
        return Err(Error::bad("邀请测试额度须为100至5000点"));
    }
    let mut conn = app.store.conn()?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let account: Option<String> = tx
        .query_row(
            "SELECT user_id FROM accounts WHERE email=? AND disabled=0",
            [&email],
            |r| r.get(0),
        )
        .optional()?;
    let pending = if let Some(user) = account {
        apply(&tx, &user, credits)?;
        false
    } else {
        let changed = tx.execute(
            "UPDATE account_invites SET test_credits=? WHERE email=? AND used=0 AND expires>? AND claim_user_id IS NULL",
            params![credits, email, chrono::Utc::now().timestamp()],
        )?;
        if changed != 1 {
            return Err(Error::bad(
                "没有可调整的新用户邀请；认领旧账号的邀请不重复赠送试用额度",
            ));
        }
        true
    };
    tx.commit()?;
    Ok(pending)
}

/// Must run inside the caller's immediate transaction. Only increase the initial
/// trial allotment; preserve consumption, reservations, top-ups and expiry.
pub(super) fn apply(conn: &Connection, user: &str, credits: i64) -> Result<()> {
    let (plan, expires, granted): (String, i64, i64) = conn.query_row(
        "SELECT plan_id,expires,cycle_granted FROM billing_wallets WHERE user_id=?",
        [user],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    if plan != "trial" || expires <= chrono::Utc::now().timestamp() {
        return Err(Error::bad("只能调整有效邀请试用的初始额度"));
    }
    if credits < granted {
        return Err(Error::bad("已发放的测试点不能通过邀请额度设置扣回"));
    }
    let extra = credits - granted;
    if extra == 0 {
        return Ok(());
    }
    conn.execute(
        "UPDATE billing_wallets SET included=included+?,granted=granted+?,cycle_granted=? WHERE user_id=?",
        params![extra, extra, credits, user],
    )?;
    billing::event(
        conn,
        user,
        &format!("invitation-allotment:{credits}"),
        "grant",
        &format!("邀请加赠 · {extra}测试点"),
        extra,
    )?;
    Ok(())
}
