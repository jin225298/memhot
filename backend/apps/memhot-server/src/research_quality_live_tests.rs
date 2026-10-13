//! Explicit, one-call review regression for a completed frozen report. No web
//! calls, production writes, or replay of an incomplete receipt.
use crate::{
    agent::{self, Scope},
    App,
};
use serde_json::{json, Value};
use std::{os::unix::fs::PermissionsExt, path::PathBuf};

#[tokio::test]
#[ignore = "explicit isolated paid QA requires protected provider and frozen inputs"]
async fn source_boundary_rejects_local_proposals() {
    let dir = PathBuf::from(std::env::var("MEMHOT_BOUNDARY_QA_DIR").unwrap());
    let input = PathBuf::from(std::env::var("MEMHOT_BOUNDARY_QA_INPUT").unwrap());
    assert!(dir.is_absolute() && dir.to_string_lossy().contains("/qa/") && !dir.exists());
    let input: Value = serde_json::from_slice(&std::fs::read(input).unwrap()).unwrap();
    let app = App::open(dir.clone()).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    let (user, _) = crate::auth::create_pair(&app, "isolated source boundary QA", None).unwrap();
    let conn = app.store.conn().unwrap();
    crate::billing::ensure_wallet(&conn, &user).unwrap();
    conn.execute(
        "UPDATE billing_wallets SET included=3 WHERE user_id=?",
        [&user],
    )
    .unwrap();
    drop(conn);
    app.store
        .queue(
            &user,
            "research",
            json!({"kind":"research"}),
            "source-boundary-qa",
        )
        .unwrap();
    let conn = app.store.conn().unwrap();
    let id: String = conn
        .query_row(
            "SELECT id FROM jobs WHERE user_id=? AND dedup='source-boundary-qa'",
            [&user],
            |r| r.get(0),
        )
        .unwrap();
    conn.execute(
        "UPDATE jobs SET status='running',attempt=1,lease_until=? WHERE id=?",
        rusqlite::params![chrono::Utc::now().timestamp() + 300, id],
    )
    .unwrap();
    drop(conn);
    let job = crate::db::Job {
        id,
        user,
        kind: "research".into(),
        payload: json!({"kind":"research"}),
        attempt: 1,
    };
    let epoch = app.store.epoch(&job.user).unwrap();
    let scope = Scope::private_at(&app, &job.user, Some(&job), &job.id, epoch).unwrap();
    let result = agent::text(
        &app,
        &job,
        epoch,
        "source-boundary-qa",
        crate::research_prompts::REVIEW,
        input,
    )
    .await
    .unwrap();
    let review = crate::agent_structured::decode(
        &app,
        &scope,
        "source-boundary-qa",
        &result,
        crate::research_prompts::REVIEW,
    )
    .await
    .unwrap();
    let path = dir.join("review.json");
    std::fs::write(&path, serde_json::to_vec_pretty(&review).unwrap()).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let checks = review["checks"].as_array().unwrap();
    for index in [0, 1, 2] {
        let check = checks.iter().find(|c| c["index"] == index).unwrap();
        assert_eq!(
            check["sourceBound"], false,
            "local rule/proposal accepted at {index}"
        );
        assert!(!crate::research_claims::admissible(check));
    }
    let valid = checks.iter().find(|c| c["index"] == 3).unwrap();
    assert!(
        crate::research_claims::admissible(valid),
        "genuine MemPal result must remain useful"
    );
    app.store
        .finish(&job, &[], "Isolated source boundary QA completed")
        .unwrap();
}
