use axum::{
    extract::{FromRequestParts, State},
    Json,
};
use memhot_server::{accounts, auth, billing, id, ledger, App};
use rusqlite::params;
use serde_json::{json, Value};
struct Fixture(App);
impl Fixture {
    fn new() -> Self {
        Self(App::open(std::env::temp_dir().join(format!("memhot-accounts-{}", id()))).unwrap())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(self.0.data_dir.as_ref());
    }
}
async fn register(app: &App, email: &str, code: &str) -> memhot_server::error::Result<Json<Value>> {
    accounts::register(State(app.clone()),Json(serde_json::from_value(json!({"email":email,"password":"a strong beta password","inviteCode":code,"name":"New name","deviceName":"test"})).unwrap())).await
}
async fn authenticated(app: &App, token: &str) -> memhot_server::error::Result<auth::User> {
    let (mut parts, _) = axum::http::Request::builder()
        .header("authorization", format!("Bearer {token}"))
        .body(())
        .unwrap()
        .into_parts();
    auth::User::from_request_parts(&mut parts, app).await
}
#[tokio::test]
async fn invitation_claim_preserves_tenant_and_password_reset_revokes_sessions() {
    let f = Fixture::new();
    let app = &f.0;
    let (user, pair) = auth::create_pair(app, "Kang", None).unwrap();
    let old = auth::pair(
        State(app.clone()),
        Json(serde_json::from_value(json!({"code":pair,"deviceName":"legacy"})).unwrap()),
    )
    .await
    .unwrap()
    .0;
    app.store
        .put(
            &user,
            "source",
            &json!({"id":"private","createdAt":"before","text":"retained"}),
            None,
        )
        .unwrap();
    let old_profile = app.store.profile(&user).unwrap();
    let invite = accounts::create_invite(app, "kang@example.com", Some(&user), 7).unwrap();
    assert!(register(app, "other@example.com", &invite).await.is_err());
    let login = register(app, " KANG@example.com ", &invite)
        .await
        .unwrap()
        .0;
    assert_eq!(login["user"]["id"], user);
    assert_eq!(login["user"]["name"], "Kang");
    assert_eq!(app.store.profile(&user).unwrap(), old_profile);
    assert_eq!(
        app.store.get(&user, "source", "private").unwrap()["text"],
        "retained"
    );
    assert!(register(app, "kang@example.com", &invite).await.is_err());
    assert!(auth::create_pair(app, "Kang", Some(&user)).is_err());
    let token = login["token"].as_str().unwrap();
    assert_eq!(authenticated(app, token).await.unwrap().0, user);
    assert!(authenticated(app, old["token"].as_str().unwrap())
        .await
        .is_err());
    app.store
        .conn()
        .unwrap()
        .execute("UPDATE devices SET expires=1 WHERE user_id=?", [&user])
        .unwrap();
    assert!(authenticated(app, token).await.is_err());
    let conn = app.store.conn().unwrap();
    assert!(!conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM devices WHERE token_hash=?)",
            [memhot_server::hash(
                old["token"].as_str().unwrap().as_bytes()
            )],
            |r| r.get::<_, bool>(0)
        )
        .unwrap());
    let stored: String = conn
        .query_row(
            "SELECT password_hash FROM accounts WHERE user_id=?",
            [&user],
            |r| r.get(0),
        )
        .unwrap();
    assert!(stored.starts_with("$argon2id$"));
    drop(conn);
    let unknown=accounts::login(State(app.clone()),Json(serde_json::from_value(json!({"email":"missing@example.com","password":"a strong beta password","deviceName":"test"})).unwrap())).await.unwrap_err();
    assert_eq!(unknown.code, "invalid_credentials");
    let code = accounts::create_reset(app, "kang@example.com").unwrap();
    let reset = || {
        serde_json::from_value(json!({"email":"kang@example.com","password":"a different strong password","resetCode":code})).unwrap()
    };
    accounts::reset_password(State(app.clone()), Json(reset()))
        .await
        .unwrap();
    assert!(accounts::reset_password(State(app.clone()), Json(reset()))
        .await
        .is_err());
    assert_eq!(
        app.store
            .conn()
            .unwrap()
            .query_row(
                "SELECT count(*) FROM devices WHERE user_id=?",
                [&user],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    assert_eq!(
        billing::account(app, &user).unwrap()["quota"]["totalGranted"],
        100
    );
}
#[test]
fn concurrent_reservation_never_overdraws_and_settlement_is_once() {
    let f = Fixture::new();
    let app = &f.0;
    let (user, _) = auth::create_pair(app, "A", None).unwrap();
    billing::account(app, &user).unwrap();
    app.store
        .conn()
        .unwrap()
        .execute(
            "UPDATE billing_wallets SET included=1 WHERE user_id=?",
            [&user],
        )
        .unwrap();
    let handles: Vec<_> = (0..2)
        .map(|n| {
            let app = app.clone();
            let user = user.clone();
            std::thread::spawn(move || {
                let key = format!("request-{n}");
                let result = ledger::reserve_request(&app, &user, None, &key, "text", 1);
                (key, result)
            })
        })
        .collect();
    let outcomes: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert_eq!(outcomes.iter().filter(|(_, r)| r.is_ok()).count(), 1);
    assert_eq!(
        outcomes
            .iter()
            .filter(|(_, r)| r.as_ref().is_err_and(|e| e.code == "quota_exhausted"))
            .count(),
        1
    );
    let key = &outcomes.iter().find(|(_, r)| r.is_ok()).unwrap().0;
    ledger::settle_request(app, &user, key, &Ok("done".into())).unwrap();
    ledger::settle_request(app, &user, key, &Ok("done".into())).unwrap();
    ledger::settle_request(
        app,
        &user,
        key,
        &Err(memhot_server::error::Error::upstream(
            "truncated",
            "late conflicting result",
        )),
    )
    .unwrap();
    assert_eq!(
        ledger::reserve_request(app, &user, None, key, "text", 1).unwrap(),
        Some("done".into())
    );
    let account = billing::account(app, &user).unwrap();
    assert_eq!(account["quota"]["used"], 1);
    assert_eq!(account["quota"]["reserved"], 0);
}
#[tokio::test]
async fn independent_lanes_limit_duplicate_requests_and_topups_are_bounded() {
    let f = Fixture::new();
    let app = &f.0;
    let (a, _) = auth::create_pair(app, "A", None).unwrap();
    let (b, _) = auth::create_pair(app, "B", None).unwrap();
    use memhot_server::concurrency::Lane;
    let first = memhot_server::models::permit(app, &a, Lane::Research)
        .await
        .unwrap();
    let second = memhot_server::models::permit(app, &a, Lane::Digest)
        .await
        .unwrap();
    assert_eq!(
        memhot_server::models::permit(app, &a, Lane::Research)
            .await
            .unwrap_err()
            .code,
        "busy"
    );
    let other = memhot_server::models::permit(app, &b, Lane::Chat)
        .await
        .unwrap();
    drop((first, second, other));
    for n in 0..5 {
        billing::api::purchase(app, &a, &format!("topup-{n}"), "test_500").unwrap();
    }
    assert_eq!(
        billing::api::purchase(app, &a, "sixth", "test_500")
            .unwrap_err()
            .code,
        "test_topup_limit"
    );
    for _ in 0..20 {
        accounts::interaction_rate(app, &a).unwrap();
    }
    assert_eq!(
        accounts::interaction_rate(app, &a).unwrap_err().code,
        "interaction_rate_limited"
    );
    accounts::interaction_rate(app, &b).unwrap();
    app.store
        .conn()
        .unwrap()
        .execute(
            "UPDATE billing_wallets SET included=0,topup=0 WHERE user_id=?",
            [&a],
        )
        .unwrap();
    assert_eq!(
        billing::ensure_available(app, &a, 1).unwrap_err().code,
        "quota_exhausted"
    );
}
#[test]
fn tenant_scoped_test_orders_and_unknown_holds_survive_receipt_deletion() {
    let f = Fixture::new();
    let app = &f.0;
    let (a, _) = auth::create_pair(app, "A", None).unwrap();
    let (b, _) = auth::create_pair(app, "B", None).unwrap();
    let order = billing::api::purchase(app, &a, "same", "test_100").unwrap();
    assert_eq!(
        order,
        billing::api::purchase(app, &a, "same", "test_100").unwrap()
    );
    assert_eq!(
        billing::api::purchase(app, &a, "same", "test_500")
            .unwrap_err()
            .code,
        "idempotency_conflict"
    );
    assert_ne!(
        order,
        billing::api::purchase(app, &b, "same", "test_100").unwrap()
    );
    assert!(billing::event_json(&app.store.conn().unwrap(), &b, &order).is_err());
    ledger::reserve_request(app, &a, None, "unknown", "asr", 2).unwrap();
    ledger::settle_request(
        app,
        &a,
        "unknown",
        &Err(memhot_server::error::Error::upstream("timeout", "unknown")),
    )
    .unwrap();
    app.store
        .conn()
        .unwrap()
        .execute("DELETE FROM receipts WHERE user_id=?", [&a])
        .unwrap();
    assert_eq!(
        ledger::reserve_request(app, &a, None, "unknown", "asr", 2)
            .unwrap_err()
            .code,
        "outcome_unknown"
    );
    let reopened = App::open(app.data_dir.as_ref().clone()).unwrap();
    let account = billing::account(&reopened, &a).unwrap();
    assert_eq!(account["quota"]["reserved"], 2);
    assert_eq!(account["quota"]["totalGranted"], 200);
    assert_eq!(
        billing::account(&reopened, &b).unwrap()["quota"]["reserved"],
        0
    );
}
#[test]
fn failed_requests_refund_to_original_bucket_and_plan_upgrades_only_grant_delta() {
    let f = Fixture::new();
    let app = &f.0;
    let (user, _) = auth::create_pair(app, "A", None).unwrap();
    billing::api::subscribe(app, &user, "beta", "beta").unwrap();
    assert_eq!(
        billing::account(app, &user).unwrap()["quota"]["totalGranted"],
        300
    );
    billing::api::subscribe(app, &user, "plus", "plus").unwrap();
    assert_eq!(
        billing::account(app, &user).unwrap()["quota"]["totalGranted"],
        1000
    );
    assert!(billing::api::subscribe(app, &user, "down", "beta").is_err());
    assert!(billing::api::subscribe(app, &user, "again", "plus").is_err());
    ledger::reserve_request(app, &user, None, "reject", "text", 1).unwrap();
    ledger::settle_request(
        app,
        &user,
        "reject",
        &Err(memhot_server::error::Error::upstream(
            "provider_auth",
            "rejected",
        )),
    )
    .unwrap();
    assert_eq!(
        billing::account(app, &user).unwrap()["quota"]["available"],
        1000
    );
    ledger::reserve_request(app, &user, None, "expired", "text", 1).unwrap();
    app.store
        .conn()
        .unwrap()
        .execute(
            "UPDATE billing_wallets SET expires=0 WHERE user_id=?",
            [&user],
        )
        .unwrap();
    billing::api::subscribe(app, &user, "new", "beta").unwrap();
    ledger::settle_request(
        app,
        &user,
        "expired",
        &Err(memhot_server::error::Error::upstream(
            "provider_auth",
            "rejected",
        )),
    )
    .unwrap();
    assert_eq!(
        billing::account(app, &user).unwrap()["quota"]["available"],
        300
    );
    let pending: i64 = app
        .store
        .conn()
        .unwrap()
        .query_row(
            "SELECT reserved FROM billing_wallets WHERE user_id=?",
            params![user],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(pending, 0);
}
#[test]
fn platform_provider_cannot_be_redirected_by_a_user_profile_or_read_from_account() {
    let mut f = Fixture::new();
    f.0.hosted = std::sync::Arc::new(
        json!({"text":{"protocol":"openai","baseUrl":"https://platform.example/v1","model":"hosted-model","apiKey":"platform-secret-fixture"}}),
    );
    let app = &f.0;
    let (user, _) = auth::create_pair(app, "A", None).unwrap();
    let mut profile = app.store.profile(&user).unwrap();
    profile["models"]["text"] = json!({"protocol":"openai","baseUrl":"https://attacker.example/v1","model":"other","apiKey":"user-key-fixture"});
    app.store.save_profile(&user, &profile).unwrap();
    let configured = memhot_server::models::config(app, &user, "text").unwrap();
    assert_eq!(configured["baseUrl"], "https://platform.example/v1");
    assert_eq!(configured["apiKey"], "platform-secret-fixture");
    assert!(!billing::account(app, &user)
        .unwrap()
        .to_string()
        .contains("platform-secret-fixture"));
    assert!(!memhot_server::profile::public_profile(&profile)
        .to_string()
        .contains("user-key-fixture"));
}

#[test]
fn public_editions_require_an_active_plan_but_not_points() {
    let f = Fixture::new();
    let app = &f.0;
    let (user, _) = auth::create_pair(app, "Subscriber", None).unwrap();
    billing::ensure_subscription(app, &user).unwrap();
    billing::ensure_available(app, &user, 1).unwrap();
    app.store
        .conn()
        .unwrap()
        .execute(
            "UPDATE billing_wallets SET included=0,topup=0 WHERE user_id=?",
            [&user],
        )
        .unwrap();
    billing::ensure_subscription(app, &user).unwrap();
    assert_eq!(
        billing::ensure_available(app, &user, 1).unwrap_err().code,
        "quota_exhausted"
    );
    app.store
        .conn()
        .unwrap()
        .execute(
            "UPDATE billing_wallets SET expires=0,topup=100 WHERE user_id=?",
            [&user],
        )
        .unwrap();
    assert_eq!(
        billing::ensure_subscription(app, &user).unwrap_err().code,
        "subscription_required"
    );
    assert_eq!(
        billing::ensure_subscription(app, "another-tenant")
            .unwrap_err()
            .code,
        "subscription_required"
    );
}
