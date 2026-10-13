use axum::{extract::State, Json};
use memhot_server::{accounts, auth, billing, id, App};
use serde_json::{json, Value};

struct Fixture(App);
impl Fixture {
    fn new() -> Self {
        Self(
            App::open(std::env::temp_dir().join(format!("memhot-invite-credits-{}", id())))
                .unwrap(),
        )
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(self.0.data_dir.as_ref());
    }
}
async fn register(app: &App, email: &str, code: &str) -> memhot_server::error::Result<Json<Value>> {
    accounts::register(
        State(app.clone()),
        Json(
            serde_json::from_value(json!({
                "email":email,"password":"a strong test password","inviteCode":code,
                "name":"Friend","deviceName":"test"
            }))
            .unwrap(),
        ),
    )
    .await
}

#[tokio::test]
async fn pending_allotment_survives_restart_and_redeems_once_without_changing_code() {
    let f = Fixture::new();
    let app = &f.0;
    let code = accounts::create_invite(app, "friend@example.com", None, 7).unwrap();
    let other = accounts::create_invite(app, "other@example.com", None, 7).unwrap();
    assert!(accounts::invitation_credits::set(app, "FRIEND@example.com", 1000).unwrap());
    assert!(accounts::invitation_credits::set(app, "friend@example.com", 1000).unwrap());
    let reopened = App::open(app.data_dir.as_ref().clone()).unwrap();
    assert!(register(&reopened, "wrong@example.com", &code)
        .await
        .is_err());
    let result = register(&reopened, "friend@example.com", &code)
        .await
        .unwrap()
        .0;
    let user = result["user"]["id"].as_str().unwrap();
    let account = billing::account(&reopened, user).unwrap();
    assert_eq!(account["quota"]["available"], 1000);
    assert_eq!(account["quota"]["totalGranted"], 1000);
    assert_eq!(account["quota"]["topup"], 0);
    assert_eq!(account["transactions"].as_array().unwrap().len(), 2);
    assert_eq!(result["user"]["name"], "Friend");
    assert!(register(&reopened, "friend@example.com", &code)
        .await
        .is_err());
    assert_eq!(
        billing::account(&reopened, user).unwrap()["quota"]["available"],
        1000
    );
    let other = register(&reopened, "other@example.com", &other)
        .await
        .unwrap()
        .0;
    assert_eq!(
        billing::account(&reopened, other["user"]["id"].as_str().unwrap()).unwrap()["quota"]
            ["available"],
        100
    );
}

#[tokio::test]
async fn already_registered_allotment_preserves_spending_reservations_and_is_idempotent() {
    let f = Fixture::new();
    let app = &f.0;
    let code = accounts::create_invite(app, "friend@example.com", None, 7).unwrap();
    let result = register(app, "friend@example.com", &code).await.unwrap().0;
    let user = result["user"]["id"].as_str().unwrap();
    app.store.conn().unwrap().execute(
        "UPDATE billing_wallets SET included=90,spent=7,reserved=3,topup=50,granted=150 WHERE user_id=?", [user],
    ).unwrap();
    let before = billing::account(app, user).unwrap();
    assert!(!accounts::invitation_credits::set(app, "friend@example.com", 1000).unwrap());
    assert!(!accounts::invitation_credits::set(app, "friend@example.com", 1000).unwrap());
    let after = billing::account(app, user).unwrap();
    assert_eq!(after["quota"]["available"], 1040);
    assert_eq!(after["quota"]["totalGranted"], 1050);
    assert_eq!(after["quota"]["used"], 7);
    assert_eq!(after["quota"]["reserved"], 3);
    assert_eq!(after["quota"]["topup"], 50);
    assert_eq!(after["subscription"], before["subscription"]);
    assert!(accounts::invitation_credits::set(app, "friend@example.com", 100).is_err());
    assert_eq!(billing::account(app, user).unwrap(), after);
}

#[test]
fn invalid_expired_and_legacy_claim_invites_are_not_modified() {
    let f = Fixture::new();
    let app = &f.0;
    let code = accounts::create_invite(app, "expired@example.com", None, 7).unwrap();
    app.store
        .conn()
        .unwrap()
        .execute(
            "UPDATE account_invites SET expires=0 WHERE code_hash=?",
            [memhot_server::hash(code.as_bytes())],
        )
        .unwrap();
    let (legacy, _) = auth::create_pair(app, "legacy", None).unwrap();
    accounts::create_invite(app, "legacy@example.com", Some(&legacy), 7).unwrap();
    accounts::create_invite(app, "valid@example.com", None, 7).unwrap();
    for email in [
        "expired@example.com",
        "legacy@example.com",
        "missing@example.com",
    ] {
        assert!(accounts::invitation_credits::set(app, email, 1000).is_err());
    }
    for credits in [-1, 99, 5001, i64::MAX] {
        assert!(accounts::invitation_credits::set(app, "valid@example.com", credits).is_err());
    }
    let changed: i64 = app
        .store
        .conn()
        .unwrap()
        .query_row(
            "SELECT count(*) FROM account_invites WHERE test_credits<>100",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(changed, 0);
}

#[test]
fn old_invitation_schema_migrates_without_losing_existing_codes() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE account_invites(code_hash TEXT PRIMARY KEY,email TEXT NOT NULL,claim_user_id TEXT,expires INTEGER NOT NULL,used INTEGER NOT NULL DEFAULT 0,created TEXT NOT NULL);
        INSERT INTO account_invites VALUES('existing-hash','friend@example.com',NULL,100000,0,'before');").unwrap();
    accounts::invitation_credits::migrate(&conn).unwrap();
    accounts::invitation_credits::migrate(&conn).unwrap();
    let row: (String, i64) = conn
        .query_row(
            "SELECT code_hash,test_credits FROM account_invites",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(row, ("existing-hash".into(), 100));
}
