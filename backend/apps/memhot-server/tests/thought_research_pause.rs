mod common;
use common::*;
use memhot_server::{agent, db::Job, ledger, now, research, App};
use rusqlite::params;
use serde_json::{json, Value};

fn thought(app: &App, user: &str, key: &str) {
    app.store.put(user,"thought",&json!({"id":key,"title":"一个问题","question":"怎样找到证据？","summary":"原始事实",
        "status":"open","researchEligible":true,"analysisStatus":"ready","revision":1,"createdAt":now()}),None).unwrap();
}
fn queue(app: &App, user: &str, thought: &str, key: &str) -> String {
    app.store
        .queue(
            user,
            "research",
            json!({"thoughtId":thought,"kind":"research"}),
            key,
        )
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned()
}
fn status(app: &App, user: &str, key: &str) -> String {
    app.store
        .conn()
        .unwrap()
        .query_row(
            "SELECT status FROM jobs WHERE user_id=? AND id=?",
            params![user, key],
            |r| r.get(0),
        )
        .unwrap()
}
fn report(key: &str, thought: &str) -> Value {
    json!({"id":key,"kind":"research","thoughtId":thought,"title":"归档之后不能发布","createdAt":now()})
}

#[tokio::test]
async fn archive_atomically_cancels_queued_running_and_blocked_research_for_only_one_tenant_thought(
) {
    let f = Fixture::new();
    let (a, token) = user(&f.app, "A").await;
    let (b, _) = user(&f.app, "B").await;
    for (user, key) in [(&a, "same"), (&a, "other"), (&b, "same")] {
        thought(&f.app, user, key);
    }
    let running_id = queue(&f.app, &a, "same", "running");
    let running = f.app.store.claim().unwrap().unwrap();
    assert_eq!(running.id, running_id);
    let queued = queue(&f.app, &a, "same", "queued");
    let blocked = queue(&f.app, &a, "same", "blocked");
    f.app
        .store
        .conn()
        .unwrap()
        .execute("UPDATE jobs SET status='blocked' WHERE id=?", [&blocked])
        .unwrap();
    let other = queue(&f.app, &a, "other", "other");
    let foreign_id = queue(&f.app, &b, "same", "foreign");
    let foreign = f.app.store.claim().unwrap().unwrap();
    assert_eq!(foreign.id, foreign_id);
    let formation = f
        .app
        .store
        .queue(&a, "formation", json!({"thoughtId":"same"}), "formation")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let epoch = f.app.store.epoch(&a).unwrap();
    let (code, archived) = request(
        &f.app,
        "POST",
        "/api/v1/thoughts/same/feedback",
        &token,
        Some(json!({"action":"dismiss","revision":1})),
    )
    .await;
    assert_eq!(code, 200);
    assert_eq!(archived["status"], "dismissed");
    assert_eq!(archived["researchEligible"], false);
    for key in [&running_id, &queued, &blocked] {
        assert_eq!(status(&f.app, &a, key), "cancelled");
        let lease: i64 = f
            .app
            .store
            .conn()
            .unwrap()
            .query_row("SELECT lease_until FROM jobs WHERE id=?", [key], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(lease, 0);
    }
    assert_eq!(status(&f.app, &a, &other), "queued");
    assert_eq!(status(&f.app, &a, &formation), "queued");
    assert_eq!(status(&f.app, &b, &foreign_id), "running");
    assert_eq!(
        f.app.store.get(&b, "thought", "same").unwrap()["status"],
        "open"
    );
    assert_eq!(
        f.app.store.epoch(&a).unwrap(),
        epoch,
        "archiving must not revoke unrelated personal tasks"
    );
    assert!(f.app.store.list_all(&a, "cognition").unwrap().is_empty());
    assert!(!f
        .app
        .store
        .finish(&running, &[("report", report("late", "same"))], "done")
        .unwrap());
    assert!(f
        .app
        .store
        .finish(&foreign, &[("report", report("foreign", "same"))], "done")
        .unwrap());
    assert!(f.app.store.get(&a, "report", "late").is_err());
    assert!(f.app.store.get(&b, "report", "foreign").is_ok());
}

#[tokio::test]
async fn paused_running_work_cannot_spend_publish_or_replay_and_restoring_does_not_resume_it() {
    let f = Fixture::new();
    let (a, token) = user(&f.app, "A").await;
    thought(&f.app, &a, "t");
    let key = queue(&f.app, &a, "t", "research");
    let job = f.app.store.claim().unwrap().unwrap();
    assert!(ledger::reserve(&f.app, &job, "in-flight-model")
        .unwrap()
        .is_none());
    let (code, _) = request(
        &f.app,
        "PUT",
        "/api/v1/thoughts/t",
        &token,
        Some(json!({"status":"resolved","revision":1})),
    )
    .await;
    assert_eq!(code, 200);
    assert_eq!(status(&f.app, &a, &key), "cancelled");
    // Match the existing model boundary: a response arriving after revocation is
    // rejected, and its uncertain paid operation remains held instead of replayed.
    ledger::settle(
        &f.app,
        &job,
        "in-flight-model",
        &Err(memhot_server::error::Error::missing()),
    )
    .unwrap();
    let receipt: (String, Option<String>) = f
        .app
        .store
        .conn()
        .unwrap()
        .query_row(
            "SELECT status,response FROM receipts WHERE user_id=? AND key='in-flight-model'",
            [&a],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(receipt, ("pending".to_owned(), None));
    let billing: String = f
        .app
        .store
        .conn()
        .unwrap()
        .query_row(
            "SELECT state FROM billing_usage WHERE user_id=? AND operation_key='in-flight-model'",
            [&a],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(billing, "unknown");
    assert_eq!(
        ledger::reserve(&f.app, &job, "in-flight-model")
            .unwrap_err()
            .code,
        "not_found"
    );
    assert_eq!(
        ledger::reserve(&f.app, &job, "another-model")
            .unwrap_err()
            .code,
        "not_found"
    );
    assert_eq!(
        research::run(&f.app, &job).await.unwrap_err().code,
        "not_found"
    );
    assert_eq!(
        agent::text(
            &f.app,
            &job,
            f.app.store.epoch(&a).unwrap(),
            "after-archive",
            "ignored",
            json!({})
        )
        .await
        .unwrap_err()
        .code,
        "not_found"
    );
    assert!(!f
        .app
        .store
        .finish(&job, &[("report", report("r", "t"))], "done")
        .unwrap());
    f.app
        .store
        .fail(
            &job,
            &memhot_server::error::Error::upstream("timeout", "late failure"),
        )
        .unwrap();
    assert_eq!(
        status(&f.app, &a, &key),
        "cancelled",
        "late failure cannot requeue a cancelled attempt"
    );
    assert_eq!(
        request(
            &f.app,
            "POST",
            "/api/v1/thoughts/t/feedback",
            &token,
            Some(json!({"action":"keep","revision":2}))
        )
        .await
        .0,
        200
    );
    assert_eq!(status(&f.app, &a, &key), "cancelled");
    assert_eq!(
        ledger::reserve(&f.app, &job, "in-flight-model")
            .unwrap_err()
            .code,
        "not_found"
    );
    let usage: i64 = f
        .app
        .store
        .conn()
        .unwrap()
        .query_row(
            "SELECT count(*) FROM billing_usage WHERE user_id=?",
            [&a],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(usage, 1);
    assert!(f.app.store.get(&a, "report", "r").is_err());
    assert_eq!(
        f.app.store.get(&a, "thought", "t").unwrap()["summary"],
        "原始事实"
    );
}

#[tokio::test]
async fn archived_state_is_checked_even_for_legacy_jobs_not_cancelled_by_an_old_writer() {
    let f = Fixture::new();
    let (a, _) = user(&f.app, "A").await;
    thought(&f.app, &a, "t");
    queue(&f.app, &a, "t", "legacy");
    let job = f.app.store.claim().unwrap().unwrap();
    f.app.store.conn().unwrap().execute("UPDATE records SET data=json_set(data,'$.status','dismissed','$.researchEligible',json('false')) WHERE user_id=? AND kind='thought' AND id='t'",[&a]).unwrap();
    assert_eq!(status(&f.app, &a, &job.id), "running");
    assert_eq!(
        research::run(&f.app, &job).await.unwrap_err().code,
        "not_found"
    );
    assert_eq!(
        ledger::reserve(&f.app, &job, "model").unwrap_err().code,
        "not_found"
    );
    assert!(agent::Scope::private(&f.app, &a, Some(&job), "legacy").is_err());
    assert!(!f
        .app
        .store
        .checkpoint(&job, &[("report", report("legacy", "t"))])
        .unwrap());
    assert!(f.app.store.list_all(&a, "report").unwrap().is_empty());
    let usage: i64 = f
        .app
        .store
        .conn()
        .unwrap()
        .query_row(
            "SELECT count(*) FROM billing_usage WHERE user_id=?",
            [&a],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(usage, 0, "status checks precede billing and network access");
}

#[tokio::test]
async fn final_publication_rechecks_the_report_thought_inside_its_transaction() {
    let f = Fixture::new();
    let (a, _) = user(&f.app, "A").await;
    thought(&f.app, &a, "t");
    // A publication carrying a topic reference must check that reference even if
    // a legacy job has no thoughtId in its payload.
    f.app
        .store
        .queue(&a, "research", json!({}), "legacy-unscoped")
        .unwrap();
    let job: Job = f.app.store.claim().unwrap().unwrap();
    f.app.store.conn().unwrap().execute("UPDATE records SET data=json_set(data,'$.status','resolved') WHERE user_id=? AND kind='thought' AND id='t'",[&a]).unwrap();
    assert!(!f
        .app
        .store
        .finish(
            &job,
            &[
                (
                    "raw",
                    json!({"id":"page","text":"paid result","createdAt":now()})
                ),
                ("report", report("late", "t"))
            ],
            "done"
        )
        .unwrap());
    assert!(
        f.app.store.get(&a, "raw", "page").is_err(),
        "reject the whole publication atomically"
    );
    assert!(f.app.store.get(&a, "report", "late").is_err());
}

#[tokio::test]
async fn restoring_then_explicitly_requesting_materials_creates_a_fresh_job_without_replaying_cancelled_work(
) {
    let f = Fixture::new();
    let (a, token) = user(&f.app, "A").await;
    thought(&f.app, &a, "t");
    let (code, first) = request(
        &f.app,
        "POST",
        "/api/v1/research",
        &token,
        Some(json!({"thoughtId":"t","kind":"research"})),
    )
    .await;
    assert_eq!(code, 202);
    let old = f.app.store.claim().unwrap().unwrap();
    assert_eq!(first["id"], old.id);
    ledger::reserve(&f.app, &old, "old-model").unwrap();
    assert_eq!(
        request(
            &f.app,
            "POST",
            "/api/v1/thoughts/t/feedback",
            &token,
            Some(json!({"action":"dismiss","revision":1}))
        )
        .await
        .0,
        200
    );
    assert_eq!(
        request(
            &f.app,
            "POST",
            "/api/v1/research",
            &token,
            Some(json!({"thoughtId":"t","kind":"research"}))
        )
        .await
        .0,
        400
    );
    assert_eq!(
        request(
            &f.app,
            "POST",
            "/api/v1/thoughts/t/feedback",
            &token,
            Some(json!({"action":"keep","revision":2}))
        )
        .await
        .0,
        200
    );
    let (code, fresh) = request(
        &f.app,
        "POST",
        "/api/v1/research",
        &token,
        Some(json!({"thoughtId":"t","kind":"research"})),
    )
    .await;
    assert_eq!(code, 202);
    assert_ne!(fresh["id"], old.id);
    assert_eq!(status(&f.app, &a, &old.id), "cancelled");
    assert_eq!(status(&f.app, &a, fresh["id"].as_str().unwrap()), "queued");
    assert_eq!(
        ledger::reserve(&f.app, &old, "old-model").unwrap_err().code,
        "not_found"
    );
    let fresh_job = loop {
        let claimed = f.app.store.claim().unwrap().unwrap();
        if claimed.id == fresh["id"].as_str().unwrap() {
            break claimed;
        }
        assert!(f.app.store.finish(&claimed, &[], "done").unwrap());
    };
    assert!(ledger::reserve(&f.app, &fresh_job, "fresh-explicit-model")
        .unwrap()
        .is_none());
    assert!(f
        .app
        .store
        .finish(&fresh_job, &[("report", report("fresh", "t"))], "done")
        .unwrap());
    assert!(f.app.store.get(&a, "report", "fresh").is_ok());
    assert!(!f
        .app
        .store
        .finish(&old, &[("report", report("old", "t"))], "done")
        .unwrap());
    assert!(f.app.store.get(&a, "report", "old").is_err());
}
