mod common;
use common::{request, user, Fixture};
use memhot_server::agent::{self, Scope};
use serde_json::json;

#[tokio::test]
async fn prepared_private_input_keeps_its_original_epoch_across_deletion_and_retry() {
    let f = Fixture::new();
    let (owner, token) = user(&f.app, "owner").await;
    let (_, preference) = request(
        &f.app,
        "POST",
        "/api/v1/cognition",
        &token,
        Some(json!({"type":"preference","statement":"private prepared context"})),
    )
    .await;
    let prepared_epoch = f.app.store.epoch(&owner).unwrap();
    let prepared_input = json!({"privatePreference":preference["statement"]});
    f.app
        .store
        .queue(&owner, "digest", json!({}), "prepared-run")
        .unwrap();
    let job = f.app.store.claim().unwrap().unwrap();
    let prepared_scope =
        Scope::private_at(&f.app, &owner, Some(&job), &job.id, prepared_epoch).unwrap();
    assert_eq!(
        request(
            &f.app,
            "DELETE",
            &format!("/api/v1/cognition/{}", preference["id"].as_str().unwrap()),
            &token,
            None,
        )
        .await
        .0,
        204
    );
    // A restart must not rebase the already prepared prompt onto the newer epoch.
    for _ in 0..2 {
        let outcome = Scope::private_at(&f.app, &owner, Some(&job), &job.id, prepared_epoch);
        assert_eq!(outcome.err().unwrap().code, "context_deleted");
        let outcome = agent::text(
            &f.app,
            &job,
            prepared_epoch,
            "prepared-review",
            "Review the supplied input.",
            prepared_input.clone(),
        )
        .await;
        assert_eq!(outcome.err().unwrap().code, "context_deleted");
        // Deletion after scope creation must also reject checkpoint creation and paid I/O.
        let outcome = agent::run(
            &f.app,
            &prepared_scope,
            agent::Spec {
                mode: "review",
                purpose: "prepared-review",
                instructions: "Review the supplied input.",
                prompt: prepared_input.clone(),
                max_steps: 1,
                max_output_tokens: 100,
                tools: false,
            },
            agent::ToolContext::default(),
        )
        .await;
        assert_eq!(outcome.err().unwrap().code, "context_deleted");
    }
    assert!(f.app.store.list(&owner, "agent_run").unwrap().is_empty());
    let receipts: i64 = f
        .app
        .store
        .conn()
        .unwrap()
        .query_row(
            "SELECT count(*) FROM receipts WHERE user_id=?",
            [&owner],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(receipts, 0);
    assert!(Scope::private_at(
        &f.app,
        &owner,
        Some(&job),
        "fresh-input",
        f.app.store.epoch(&owner).unwrap(),
    )
    .is_ok());
}
