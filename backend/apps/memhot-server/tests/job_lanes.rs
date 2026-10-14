use memhot_server::{auth, concurrency::Lane, db::Job, error::Error, App};
use rusqlite::params;
use serde_json::json;
use std::sync::{Arc, Barrier};

struct Fixture(App);
impl Fixture {
    fn new() -> Self {
        Self(
            App::open(std::env::temp_dir().join(format!("memhot-lanes-{}", memhot_server::id())))
                .unwrap(),
        )
    }
    fn user(&self, name: &str) -> String {
        auth::create_pair(&self.0, name, None).unwrap().0
    }
    fn queue(&self, user: &str, kind: &str, dedup: &str) {
        self.0.store.queue(user, kind, json!({}), dedup).unwrap();
    }
    fn claim(&self, lane: Lane) -> Job {
        self.0.store.claim_lane(lane).unwrap().unwrap()
    }
    fn reopen(&self) -> App {
        App::open(self.0.data_dir.as_ref().clone()).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(self.0.data_dir.as_ref());
    }
}

#[test]
fn same_user_has_three_independent_lanes_and_waiting_jobs_keep_their_attempts() {
    let f = Fixture::new();
    let user = f.user("A");
    for (kind, lane) in [
        ("formation", Lane::Research),
        ("digest", Lane::Digest),
        ("material_translation", Lane::Translation),
    ] {
        f.queue(&user, kind, &format!("{kind}-1"));
        let running = f.claim(lane);
        assert_eq!(running.kind, kind);
        f.queue(&user, kind, &format!("{kind}-2"));
        assert!(f.0.store.claim_lane(lane).unwrap().is_none());
    }
    assert!(f.0.store.claim_lane(Lane::Chat).unwrap().is_none());
    let conn = f.0.store.conn().unwrap();
    let (running, untouched): (i64, i64) = conn.query_row(
        "SELECT SUM(status='running'),SUM(status='queued' AND attempt=0 AND lease_until=0) FROM jobs",
        [], |r| Ok((r.get(0)?, r.get(1)?)),
    ).unwrap();
    assert_eq!((running, untouched), (3, 3));
}

#[test]
fn generic_claim_serializes_related_research_but_allows_other_lanes() {
    let f = Fixture::new();
    let user = f.user("A");
    for kind in [
        "formation",
        "cognition",
        "research",
        "daily_summary",
        "transcribe",
    ] {
        f.queue(&user, kind, kind);
    }
    let first = f.0.store.claim().unwrap().unwrap();
    assert_eq!(Lane::for_job(&first.kind), Lane::Research);
    assert!(f.0.store.claim().unwrap().is_none());
    f.queue(&user, "digest", "digest");
    f.queue(&user, "material_translation", "translate");
    let second = f.0.store.claim().unwrap().unwrap();
    let third = f.0.store.claim().unwrap().unwrap();
    assert_ne!(second.kind, third.kind);
    assert!(f.0.store.claim().unwrap().is_none());
}

fn race_claims(f: &Fixture, lane: Lane) -> Vec<Job> {
    let apps = [f.reopen(), f.reopen()];
    let barrier = Arc::new(Barrier::new(2));
    let threads: Vec<_> = apps
        .into_iter()
        .map(|app| {
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                app.store.claim_lane(lane).unwrap()
            })
        })
        .collect();
    threads
        .into_iter()
        .filter_map(|t| t.join().unwrap())
        .collect()
}

#[test]
fn lane_capacity_is_atomic_across_connections_and_stale_workers_cannot_commit() {
    let f = Fixture::new();
    let a = f.user("A");
    let b = f.user("B");
    f.queue(&a, "research", "a");
    f.queue(&b, "research", "b");
    let mut jobs = race_claims(&f, Lane::Research);
    assert_eq!(
        jobs.len(),
        1,
        "separate workers share one durable lane slot"
    );
    let old = jobs.pop().unwrap();
    // Keep the other tenant queued but not due, isolating recovery of the expired lease.
    f.0.store
        .conn()
        .unwrap()
        .execute(
            "UPDATE jobs SET next_at=? WHERE id<>?",
            params![chrono::Utc::now().timestamp() + 3600, old.id],
        )
        .unwrap();
    f.0.store
        .conn()
        .unwrap()
        .execute("UPDATE jobs SET lease_until=0 WHERE id=?", [&old.id])
        .unwrap();
    let mut recovered = race_claims(&f, Lane::Research);
    assert_eq!(recovered.len(), 1);
    let current = recovered.pop().unwrap();
    assert_eq!(current.id, old.id);
    assert_eq!(current.attempt, 2);
    assert!(!f.0.store.finish(&old, &[], "stale").unwrap());
    f.0.store
        .fail(&old, &Error::upstream("timeout", "stale"))
        .unwrap();
    assert!(!f.0.store.checkpoint(&old, &[]).unwrap());
    let status: String =
        f.0.store
            .conn()
            .unwrap()
            .query_row("SELECT status FROM jobs WHERE id=?", [&current.id], |r| {
                r.get(0)
            })
            .unwrap();
    assert_eq!(status, "running");
    assert!(f.0.store.finish(&current, &[], "done").unwrap());
}

#[test]
fn each_lane_rotates_tenants_durably_without_other_lanes_spending_their_turn() {
    let f = Fixture::new();
    let a = f.user("A");
    let b = f.user("B");
    for (kind, lane) in [
        ("research", Lane::Research),
        ("digest", Lane::Digest),
        ("material_translation", Lane::Translation),
    ] {
        f.queue(&a, kind, &format!("{kind}-a1"));
        let first = f.claim(lane);
        assert_eq!(first.user, a);
        f.0.store.finish(&first, &[], "done").unwrap();
        f.queue(&a, kind, &format!("{kind}-a2"));
        f.queue(&b, kind, &format!("{kind}-b1"));
    }
    let reopened = f.reopen();
    for lane in [Lane::Translation, Lane::Digest, Lane::Research] {
        let next = reopened.store.claim_lane(lane).unwrap().unwrap();
        assert_eq!(next.user, b);
        reopened.store.finish(&next, &[], "done").unwrap();
        let next = reopened.store.claim_lane(lane).unwrap().unwrap();
        assert_eq!(next.user, a);
    }
}

#[test]
fn old_scheduler_turns_migrate_once_without_overwriting_lane_progress() {
    let f = Fixture::new();
    let a = f.user("A");
    let b = f.user("B");
    f.0.store
        .conn()
        .unwrap()
        .execute(
            "INSERT INTO scheduler_turns(user_id,last_turn) VALUES(?,42)",
            [&a],
        )
        .unwrap();
    // Simulate an existing deployment before the lane table was introduced.
    f.0.store
        .conn()
        .unwrap()
        .execute_batch("DROP TABLE scheduler_lane_turns")
        .unwrap();
    let reopened = f.reopen();
    let turns: i64 = reopened
        .store
        .conn()
        .unwrap()
        .query_row(
            "SELECT count(*) FROM scheduler_lane_turns WHERE user_id=? AND last_turn=42",
            [&a],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(turns, 3);
    f.queue(&a, "digest", "a");
    f.queue(&b, "digest", "b");
    let next = reopened.store.claim_lane(Lane::Digest).unwrap().unwrap();
    assert_eq!(next.user, b);
    reopened.store.finish(&next, &[], "done").unwrap();
    let reopened = f.reopen();
    let turn: i64 = reopened
        .store
        .conn()
        .unwrap()
        .query_row(
            "SELECT last_turn FROM scheduler_lane_turns WHERE user_id=? AND lane='digest'",
            [&b],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(turn, 43);
}

#[test]
fn newest_digest_delivery_is_first_but_recovery_still_precedes_new_paid_work() {
    let f = Fixture::new();
    let user = f.user("A");
    for date in ["2026-10-12", "2026-10-14", "2026-10-13"] {
        f.0.store
            .queue(&user, "digest", json!({"researchDate":date}), date)
            .unwrap();
    }
    let newest = f.claim(Lane::Digest);
    assert_eq!(newest.payload["researchDate"], "2026-10-14");
    f.0.store
        .conn()
        .unwrap()
        .execute("UPDATE jobs SET lease_until=0 WHERE id=?", [&newest.id])
        .unwrap();
    f.0.store
        .queue(
            &user,
            "digest",
            json!({"researchDate":"2026-10-15"}),
            "newer",
        )
        .unwrap();
    let recovered = f.claim(Lane::Digest);
    assert_eq!(recovered.id, newest.id);
    f.0.store.finish(&recovered, &[], "done").unwrap();
    assert_eq!(f.claim(Lane::Digest).payload["researchDate"], "2026-10-15");
}

#[test]
fn exhausted_lease_is_not_reclaimed_and_queue_dedup_survives_parallel_lanes() {
    let f = Fixture::new();
    let user = f.user("A");
    f.queue(&user, "material_translation", "same");
    let first = f.claim(Lane::Translation);
    f.queue(&user, "material_translation", "same");
    f.0.store
        .conn()
        .unwrap()
        .execute(
            "UPDATE jobs SET attempt=3,lease_until=0 WHERE id=?",
            [&first.id],
        )
        .unwrap();
    assert!(f.0.store.claim_lane(Lane::Translation).unwrap().is_none());
    let count: i64 =
        f.0.store
            .conn()
            .unwrap()
            .query_row("SELECT count(*) FROM jobs", [], |r| r.get(0))
            .unwrap();
    assert_eq!(count, 1);
    f.queue(&user, "material_translation", "other");
    assert_eq!(f.claim(Lane::Translation).attempt, 1);
}

#[test]
fn reopening_does_not_seed_other_lanes_from_new_research_turns() {
    let f = Fixture::new();
    let a = f.user("A");
    let b = f.user("B");
    f.queue(&a, "digest", "digest-a1");
    let a_digest = f.claim(Lane::Digest);
    f.0.store.finish(&a_digest, &[], "done").unwrap();
    f.queue(&b, "research", "research-b1");
    let b_research = f.claim(Lane::Research);
    f.0.store.finish(&b_research, &[], "done").unwrap();
    f.queue(&a, "digest", "digest-a2");
    f.queue(&b, "digest", "digest-b1");
    let reopened = f.reopen();
    assert_eq!(
        reopened
            .store
            .claim_lane(Lane::Digest)
            .unwrap()
            .unwrap()
            .user,
        b
    );
}
