use super::*;

#[tokio::test]
async fn all_four_lanes_execute_for_one_owner_while_background_is_busy() {
    let gates = Concurrency::default();
    let mut held = Vec::new();
    for lane in [Lane::Research, Lane::Digest, Lane::Translation, Lane::Chat] {
        let pair = tokio::time::timeout(Duration::from_millis(100), async {
            let agent = gates.agent(lane, Some("same-user")).await.unwrap();
            let model = gates.model(lane, Some("same-user")).await.unwrap();
            (agent, model)
        })
        .await
        .expect("a background lane must not consume another lane's capacity");
        held.push(pair);
    }
    assert_eq!(held.len(), 4);
    assert_eq!(
        gates
            .agent(Lane::Chat, Some("same-user"))
            .await
            .unwrap_err()
            .code,
        "busy"
    );
    drop(held);
    let _again = gates
        .agent(Lane::Research, Some("same-user"))
        .await
        .unwrap();
}

#[tokio::test(start_paused = true)]
async fn waiting_and_running_cancellations_release_both_capacity_and_owner() {
    let gates = Arc::new(Concurrency::default());
    let active = gates.agent(Lane::Chat, Some("first")).await.unwrap();
    let waiter = {
        let gates = gates.clone();
        tokio::spawn(async move { gates.agent(Lane::Chat, Some("second")).await })
    };
    tokio::task::yield_now().await;
    assert_eq!(
        gates
            .agent(Lane::Chat, Some("second"))
            .await
            .unwrap_err()
            .code,
        "busy"
    );
    waiter.abort();
    assert!(waiter.await.unwrap_err().is_cancelled());
    drop(active);
    let recovered = gates.agent(Lane::Chat, Some("second")).await.unwrap();
    drop(recovered);
    let _next = gates.agent(Lane::Chat, Some("third")).await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn overload_is_bounded_without_consuming_other_lanes_and_timeout_is_recoverable() {
    let gates = Arc::new(Concurrency::default());
    let held = gates.model(Lane::Digest, Some("active")).await.unwrap();
    let mut waiters = Vec::new();
    for n in 0..16 {
        let gates = gates.clone();
        waiters.push(tokio::spawn(async move {
            gates.model(Lane::Digest, Some(&format!("owner-{n}"))).await
        }));
        tokio::task::yield_now().await;
    }
    assert_eq!(
        gates
            .model(Lane::Digest, Some("overflow"))
            .await
            .unwrap_err()
            .code,
        "busy"
    );
    let _interactive = gates.model(Lane::Chat, Some("overflow")).await.unwrap();
    tokio::time::advance(Duration::from_secs(31)).await;
    for waiter in waiters {
        assert_eq!(waiter.await.unwrap().unwrap_err().code, "busy");
    }
    drop(held);
    let _recovered = gates.model(Lane::Digest, Some("owner-0")).await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn distinct_owners_are_fifo_and_repeated_submissions_cannot_cut_in_line() {
    let gates = Arc::new(Concurrency::default());
    let held = gates.agent(Lane::Research, Some("active")).await.unwrap();
    let (tx, mut rx) = tokio::sync::mpsc::channel(3);
    let mut waiters = Vec::new();
    for n in 0..3 {
        let gates = gates.clone();
        let tx = tx.clone();
        waiters.push(tokio::spawn(async move {
            let _permit = gates
                .agent(Lane::Research, Some(&format!("user-{n}")))
                .await
                .unwrap();
            tx.send(n).await.unwrap();
        }));
        tokio::task::yield_now().await;
    }
    assert_eq!(
        gates
            .agent(Lane::Research, Some("user-2"))
            .await
            .unwrap_err()
            .code,
        "busy"
    );
    drop(held);
    for n in 0..3 {
        assert_eq!(rx.recv().await, Some(n));
    }
    for waiter in waiters {
        waiter.await.unwrap();
    }
}

#[tokio::test]
async fn conversation_owner_covers_snapshot_and_commit_without_locking_other_features() {
    let gates = Concurrency::default();
    let conversation = gates.conversation("owner").unwrap();
    let agent = gates.agent(Lane::Chat, Some("owner")).await.unwrap();
    let model = gates.model(Lane::Chat, Some("owner")).await.unwrap();
    drop((agent, model));
    assert_eq!(gates.conversation("owner").unwrap_err().code, "busy");
    let _translation = gates.agent(Lane::Translation, Some("owner")).await.unwrap();
    let _other_user = gates.conversation("other").unwrap();
    drop(conversation);
    assert!(gates.conversation("owner").is_ok());
}

#[tokio::test]
async fn model_failure_or_completed_result_does_not_leave_an_execution_slot_held() {
    let gates = Concurrency::default();
    async fn attempt(gates: &Concurrency) -> Result<()> {
        let _permit = gates.model(Lane::Research, Some("owner")).await?;
        Err(Error::upstream("network", "fixture failed"))
    }
    assert!(attempt(&gates).await.is_err());
    let _next = gates.model(Lane::Research, Some("owner")).await.unwrap();
}
