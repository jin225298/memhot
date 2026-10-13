use memhot_server::{auth, billing, error::Error, id, public_usage, App};
struct Fixture(App);
impl Fixture {
    fn new() -> Self {
        Self(App::open(std::env::temp_dir().join(format!("memhot-public-usage-{}", id()))).unwrap())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(self.0.data_dir.as_ref());
    }
}
#[test]
fn shared_public_calls_replay_without_spending_any_users_credits() {
    let f = Fixture::new();
    let app = &f.0;
    let (a, _) = auth::create_pair(app, "A", None).unwrap();
    let (b, _) = auth::create_pair(app, "B", None).unwrap();
    let before_a = billing::account(app, &a).unwrap()["quota"].clone();
    let before_b = billing::account(app, &b).unwrap()["quota"].clone();
    assert_eq!(
        public_usage::reserve(app, "edition:phase", "fingerprint", 20).unwrap(),
        None
    );
    public_usage::settle(app, "edition:phase", &Ok("public output".into())).unwrap();
    assert_eq!(
        public_usage::reserve(app, "edition:phase", "fingerprint", 0).unwrap(),
        Some("public output".into())
    );
    assert_eq!(
        public_usage::reserve(app, "edition:phase", "different", 20)
            .unwrap_err()
            .code,
        "public_idempotency_conflict"
    );
    assert_eq!(billing::account(app, &a).unwrap()["quota"], before_a);
    assert_eq!(billing::account(app, &b).unwrap()["quota"], before_b);
    let calls: i64 = app
        .store
        .conn()
        .unwrap()
        .query_row("SELECT count(*) FROM public_model_attempts", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(calls, 1);
}
#[test]
fn unknown_results_never_retry_and_explicit_rejections_are_bounded_and_counted() {
    let f = Fixture::new();
    let app = &f.0;
    public_usage::reserve(app, "unknown", "x", 20).unwrap();
    public_usage::settle(app, "unknown", &Err(Error::upstream("timeout", "unknown"))).unwrap();
    assert_eq!(
        public_usage::reserve(app, "unknown", "x", 20)
            .unwrap_err()
            .code,
        "public_outcome_unknown"
    );
    for _ in 0..2 {
        public_usage::reserve(app, "reject", "y", 20).unwrap();
        public_usage::settle(
            app,
            "reject",
            &Err(Error::upstream("rate_limited", "rejected")),
        )
        .unwrap();
    }
    assert_eq!(
        public_usage::reserve(app, "reject", "y", 20)
            .unwrap_err()
            .code,
        "public_retry_exhausted"
    );
    let calls: i64 = app
        .store
        .conn()
        .unwrap()
        .query_row("SELECT count(*) FROM public_model_attempts", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(calls, 3);
    assert_eq!(
        public_usage::reserve(app, "fourth", "z", 3)
            .unwrap_err()
            .code,
        "public_daily_budget_exhausted"
    );
}
#[test]
fn last_platform_budget_slot_is_atomic_and_pending_survives_reopening() {
    let f = Fixture::new();
    let app = &f.0;
    let threads: Vec<_> = (0..2)
        .map(|n| {
            let app = app.clone();
            std::thread::spawn(move || {
                public_usage::reserve(&app, &format!("edition-{n}"), "input", 1)
            })
        })
        .collect();
    let results: Vec<_> = threads.into_iter().map(|h| h.join().unwrap()).collect();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|r| r
                .as_ref()
                .is_err_and(|e| e.code == "public_daily_budget_exhausted"))
            .count(),
        1
    );
    let key: String = app
        .store
        .conn()
        .unwrap()
        .query_row("SELECT operation_key FROM public_model_calls", [], |r| {
            r.get(0)
        })
        .unwrap();
    let reopened = App::open(app.data_dir.as_ref().clone()).unwrap();
    assert_eq!(
        public_usage::reserve(&reopened, &key, "input", 20)
            .unwrap_err()
            .code,
        "public_outcome_unknown"
    );
}
