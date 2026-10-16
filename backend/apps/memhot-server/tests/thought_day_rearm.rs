mod common;
use common::*;
use memhot_server::worker;
use rusqlite::params;
use serde_json::{json, Value};
fn today() -> String {
    chrono::Utc::now()
        .with_timezone(&chrono_tz::Asia::Shanghai)
        .date_naive()
        .to_string()
}
fn night(date: &str) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::parse_from_rfc3339(&format!("{date}T15:00:00Z"))
        .unwrap()
        .with_timezone(&chrono::Utc)
}
fn state(f: &Fixture, u: &str) -> (String, String, i64, Value) {
    f.app
        .store
        .conn()
        .unwrap()
        .query_row(
            "SELECT id,status,attempt,payload FROM jobs WHERE user_id=? AND kind='research'",
            [u],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    serde_json::from_str(&r.get::<_, String>(3)?).unwrap(),
                ))
            },
        )
        .unwrap()
}
#[tokio::test]
async fn explicit_carry_rearms_only_never_claimed_expired_research_without_payment_evidence() {
    for evidence in ["none", "receipt", "billing", "attempted"] {
        let f = Fixture::new();
        let (u, t) = user(&f.app, "A").await;
        f.app.store.put(&u,"thought",&json!({"id":"old","question":"旧问题","status":"open","revision":1,"createdAt":"2020-10-12T02:00:00Z"}),None).unwrap();
        worker::schedule_at(&f.app, night("2020-10-12")).unwrap();
        let original = state(&f, &u);
        assert_eq!(original.1, "queued");
        let conn = f.app.store.conn().unwrap();
        match evidence {
            "receipt" => {
                conn.execute("INSERT INTO receipts(user_id,job_id,key,status,created) VALUES(?,?,?,'pending',?)",params![u,original.0,"uncertain",chrono::Utc::now().timestamp()]).unwrap();
            }
            "billing" => {
                conn.execute("INSERT INTO billing_usage(user_id,operation_key,fingerprint,kind,units,state,included,topup,cycle_id,created) VALUES(?,'hashed-operation','fp','text',1,'unknown',1,0,'cycle',?)",params![u,chrono::Utc::now().timestamp()+1]).unwrap();
            }
            "attempted" => {
                conn.execute("UPDATE jobs SET attempt=1 WHERE id=?", [&original.0])
                    .unwrap();
            }
            _ => {}
        }
        drop(conn);
        worker::schedule_at(&f.app, night(&today())).unwrap();
        assert_eq!(state(&f, &u).1, "cancelled");
        // Merely editing/re-saving the question never reactivates the schedule.
        worker::schedule_at(&f.app, night(&today())).unwrap();
        assert_eq!(state(&f, &u).1, "cancelled");
        assert_eq!(
            request(
                &f.app,
                "POST",
                "/api/v1/thoughts/old/today",
                &t,
                Some(json!({"revision":1}))
            )
            .await
            .0,
            200
        );
        worker::schedule_at(&f.app, night(&today())).unwrap();
        let after = state(&f, &u);
        assert_eq!(after.0, original.0);
        assert_eq!(after.2, if evidence == "attempted" { 1 } else { 0 });
        assert_eq!(
            after.1,
            if evidence == "none" {
                "queued"
            } else {
                "cancelled"
            },
            "{evidence}"
        );
        if evidence == "none" {
            assert_eq!(after.3["researchDate"], today());
            assert!(after.3["expiredSchedule"].is_null());
        } else if evidence == "receipt" {
            let n: i64 = f
                .app
                .store
                .conn()
                .unwrap()
                .query_row(
                    "SELECT count(*) FROM receipts WHERE job_id=?",
                    [&original.0],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(n, 1);
        }
    }
}
