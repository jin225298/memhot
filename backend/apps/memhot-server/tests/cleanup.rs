mod common;
use common::*;
use memhot_server::{now, sources, worker, App};
use rusqlite::params;
use serde_json::{json, Value};

fn put(app: &App, user: &str, kind: &str, mut value: Value) {
    if value["createdAt"].is_null() {
        value["createdAt"] = json!(now());
    }
    app.store.put(user, kind, &value, None).unwrap();
}
#[tokio::test]
async fn deleting_one_recording_preserves_independent_topic_evidence_and_revokes_private_closure() {
    let f = Fixture::new();
    let (user, _) = user(&f.app, "A").await;
    put(
        &f.app,
        &user,
        "source",
        json!({"id":"old","text":"PRIVATE_OLD_DETAIL","status":"ready"}),
    );
    put(
        &f.app,
        &user,
        "source",
        json!({"id":"new","text":"remaining original evidence","status":"ready","analysisCoverage":{"complete":true,"includedChars":20}}),
    );
    put(
        &f.app,
        &user,
        "thought",
        json!({"id":"topic","sourceId":"old","sourceIds":["old","new"],"status":"open","title":"我的独立标题","question":"PRIVATE_OLD_DETAIL?","summary":"PRIVATE_OLD_DETAIL mixed","editedFields":["title"],"revision":2,"evidenceRefs":[{"id":"e1","sourceId":"old","quote":"PRIVATE_OLD_DETAIL"},{"id":"e2","sourceId":"new","quote":"remaining original evidence","summary":"PRIVATE_OLD_DETAIL combined"}]}),
    );
    put(
        &f.app,
        &user,
        "report",
        json!({"id":"report","thoughtId":"topic","question":"PRIVATE_OLD_DETAIL?","futurePrivateField":"PRIVATE_OLD_DETAIL","body":"PRIVATE_OLD_DETAIL","sources":[{"url":"https://example.org/public","excerpt":"public original evidence","citations":[{"claim":"PRIVATE_OLD_DETAIL","quote":"public original evidence"}],"futurePrivateSourceField":"PRIVATE_OLD_DETAIL"}]}),
    );
    put(
        &f.app,
        &user,
        "conversation",
        json!({"id":"conv","reportId":"report","messages":[{"text":"PRIVATE_OLD_DETAIL"}]}),
    );
    put(
        &f.app,
        &user,
        "observation",
        json!({"id":"obs","conversationId":"conv","text":"PRIVATE_OLD_DETAIL"}),
    );
    put(
        &f.app,
        &user,
        "cognition",
        json!({"id":"auto","statement":"PRIVATE_OLD_DETAIL","origin":"inferred","evidenceRefs":[{"conversationId":"conv","quote":"PRIVATE_OLD_DETAIL"}]}),
    );
    put(
        &f.app,
        &user,
        "cognition",
        json!({"id":"manual","statement":"我自己确认的偏好","origin":"manual","userEdited":true,"evidenceRefs":[{"sourceId":"old","quote":"PRIVATE_OLD_DETAIL"},{"sourceId":"new","quote":"remaining original evidence"}]}),
    );
    f.app
        .store
        .queue(&user, "cognition", json!({"observationId":"obs"}), "cog")
        .unwrap();
    f.app
        .store
        .queue(&user, "formation", json!({"sourceId":"new"}), "old-form")
        .unwrap();
    let stale = f.app.store.claim().unwrap().unwrap();
    sources::delete_owned(&f.app, &user, "old").unwrap();
    let topic = f.app.store.get(&user, "thought", "topic").unwrap();
    assert_eq!(topic["title"], "我的独立标题");
    assert_eq!(topic["sourceIds"], json!(["new"]));
    assert_eq!(topic["evidenceRefs"][0]["id"], "e2");
    assert!(topic["evidenceRefs"][0].get("summary").is_none());
    assert!(topic.get("question").is_none());
    assert_eq!(topic["analysisStatus"], "pending");
    assert!(f
        .app
        .store
        .get(&user, "tombstone", "thought:topic")
        .is_err());
    for (kind, key) in [
        ("observation", "obs"),
        ("cognition", "auto"),
        ("conversation", "conv"),
    ] {
        assert!(f.app.store.get(&user, kind, key).is_err());
    }
    let manual = f.app.store.get(&user, "cognition", "manual").unwrap();
    assert_eq!(manual["statement"], "我自己确认的偏好");
    assert_eq!(manual["evidenceRefs"].as_array().unwrap().len(), 1);
    let report = f.app.store.get(&user, "report", "report").unwrap();
    assert_eq!(report["personalContextDeleted"], true);
    assert!(report.get("question").is_none());
    assert!(report.get("futurePrivateField").is_none());
    let forbidden: i64 = f
        .app
        .store
        .conn()
        .unwrap()
        .query_row(
            "SELECT count(*) FROM records WHERE user_id=? AND data LIKE '%PRIVATE_OLD_DETAIL%'",
            [&user],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(forbidden, 0);
    assert!(!f
        .app
        .store
        .finish(
            &stale,
            &[(
                "cognition",
                json!({"id":"late","evidenceRefs":[{"conversationId":"conv"}]})
            )],
            "late"
        )
        .unwrap());
    let jobs:i64=f.app.store.conn().unwrap().query_row("SELECT count(*) FROM jobs WHERE user_id=? AND kind='formation' AND json_extract(payload,'$.sourceId')='new' AND status='queued'",[&user],|r|r.get(0)).unwrap();
    assert_eq!(jobs, 1);
}
#[tokio::test]
async fn automatic_retention_does_not_tombstone_a_topic_with_a_recent_source() {
    let f = Fixture::new();
    let (user, _) = user(&f.app, "A").await;
    put(
        &f.app,
        &user,
        "source",
        json!({"id":"old","text":"old","status":"ready","kind":"note","createdAt":"2000-01-01T00:00:00Z"}),
    );
    put(
        &f.app,
        &user,
        "source",
        json!({"id":"new","text":"new","status":"ready","kind":"note"}),
    );
    put(
        &f.app,
        &user,
        "thought",
        json!({"id":"topic","sourceId":"old","sourceIds":["old","new"],"status":"open","question":"automatic old question","evidenceRefs":[{"id":"new-quote","sourceId":"new","quote":"new"}]}),
    );
    worker::retention(&f.app).unwrap();
    assert!(f.app.store.get(&user, "source", "old").is_err());
    assert!(f.app.store.get(&user, "source", "new").is_ok());
    let topic = f.app.store.get(&user, "thought", "topic").unwrap();
    assert_eq!(topic["sourceIds"], json!(["new"]));
    assert_eq!(topic["researchEligible"], false);
}
#[tokio::test]
async fn three_tenants_rotate_fairly_under_old_backlog_and_across_store_restarts() {
    let f = Fixture::new();
    let mut users = Vec::new();
    for name in ["A", "B", "C"] {
        users.push(user(&f.app, name).await.0);
    }
    for (offset, user) in users.iter().enumerate() {
        for n in 0..9 {
            f.app
                .store
                .queue(user, "daily_summary", json!({}), &format!("{offset}-{n}"))
                .unwrap();
        }
        f.app
            .store
            .conn()
            .unwrap()
            .execute(
                "UPDATE jobs SET created=? WHERE user_id=?",
                params![format!("2026-01-0{}T00:00:00Z", offset + 1), user],
            )
            .unwrap();
    }
    let other = App::open(f.app.data_dir.as_ref().clone()).unwrap();
    let mut selected = Vec::new();
    for index in 0..18 {
        let app = if index % 2 == 0 { &f.app } else { &other };
        let job = app.store.claim().unwrap().unwrap();
        selected.push(job.user.clone());
        assert!(app.store.finish(&job, &[], "done").unwrap());
    }
    for cycle in selected.chunks(3) {
        for user in &users {
            assert!(
                cycle.contains(user),
                "every tenant must get a turn before an old backlog repeats"
            );
        }
    }
}
#[tokio::test]
async fn deleting_a_topic_revokes_report_discussions_and_observations_without_touching_the_source()
{
    let f = Fixture::new();
    let (uid, token) = user(&f.app, "A").await;
    put(
        &f.app,
        &uid,
        "source",
        json!({"id":"source","text":"source stays","status":"ready"}),
    );
    put(
        &f.app,
        &uid,
        "thought",
        json!({"id":"topic","sourceId":"source","status":"open","question":"topic"}),
    );
    put(
        &f.app,
        &uid,
        "report",
        json!({"id":"report","thoughtId":"topic"}),
    );
    put(
        &f.app,
        &uid,
        "conversation",
        json!({"id":"conv","reportId":"report","messages":[]}),
    );
    put(
        &f.app,
        &uid,
        "observation",
        json!({"id":"obs","conversationId":"conv","text":"private discussion"}),
    );
    put(
        &f.app,
        &uid,
        "cognition",
        json!({"id":"derived","origin":"inferred","statement":"private inference","evidenceRefs":[{"conversationId":"conv"}]}),
    );
    f.app
        .store
        .queue(&uid, "cognition", json!({"observationId":"obs"}), "obs")
        .unwrap();
    assert_eq!(
        request(&f.app, "DELETE", "/api/v1/thoughts/topic", &token, None)
            .await
            .0,
        204
    );
    for (kind, key) in [
        ("thought", "topic"),
        ("report", "report"),
        ("conversation", "conv"),
        ("observation", "obs"),
        ("cognition", "derived"),
    ] {
        assert!(f.app.store.get(&uid, kind, key).is_err());
    }
    assert!(f.app.store.get(&uid, "source", "source").is_ok());
    let jobs: i64 = f
        .app
        .store
        .conn()
        .unwrap()
        .query_row(
            "SELECT count(*) FROM jobs WHERE user_id=? AND kind='cognition'",
            [&uid],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(jobs, 0);
}
