use super::*;
fn material(index: usize) -> Material {
    Material {
        title: format!("Paper {index}"),
        url: format!("https://arxiv.org/pdf/2601.{:05}", 12345 + index),
        text: "Known paper abstract".into(),
        published: Some("2026-01-10".into()),
        discovered: Some("2026-10-12T08:00:00Z".into()),
        discovered_via: Some("https://www.alphaxiv.org/".into()),
        publication_source: "search_reported_publication",
        publisher: "arXiv · alphaXiv 检索".into(),
        fulltext: false,
        pdf: None,
    }
}
fn fixture() -> (App, Scope, state::Saved, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("memhot-paper-fence-{}", crate::id()));
    let mut app = App::open(dir.clone()).unwrap();
    let (user, _) = crate::auth::create_pair(&app, "fixture", None).unwrap();
    app.alphaxiv = std::sync::Arc::new(Some(
        serde_json::from_value(json!({"userId":user,"apiKey":"fake-private-secret"})).unwrap(),
    ));
    let scope = Scope::private(&app, &user, None, "run").unwrap();
    let saved = state::load_or_freeze(
        &app,
        &scope,
        "test",
        json!({"model":{"model":"test"},"mode":"research"}),
        Default::default(),
    )
    .unwrap();
    (app, scope, saved, dir)
}
#[tokio::test]
async fn cached_private_library_replays_without_network_and_other_users_cannot_access_it() {
    let (app, scope, mut saved, dir) = fixture();
    let args = json!({"action":"library"});
    let request = alphaxiv::request(&args).unwrap();
    let key = crate::hash(request.start().to_string().as_bytes());
    saved.context["paperCalls"] = json!({key:{"status":"complete","tool":"list_library","value":{"sources":[],"folders":[{"name":"Private fixture"}]}}});
    let response = execute(&app, &scope, "test", &mut saved, &args)
        .await
        .unwrap();
    assert_eq!(response["folders"][0]["name"], "Private fixture");
    let (other, _) = crate::auth::create_pair(&app, "other", None).unwrap();
    let other = Scope::private(&app, &other, None, "run").unwrap();
    assert_eq!(
        execute(&app, &other, "test", &mut saved, &args)
            .await
            .unwrap_err()
            .code,
        "not_found"
    );
    assert_eq!(
        execute(
            &app,
            &Scope::public("e", "ai").unwrap(),
            "test",
            &mut saved,
            &args
        )
        .await
        .unwrap_err()
        .code,
        "not_found"
    );
    drop(app);
    std::fs::remove_dir_all(dir).unwrap();
}
#[tokio::test]
async fn uncertain_receipt_survives_reload_and_search_budget_blocks_another_external_call() {
    let (app, scope, mut saved, dir) = fixture();
    let args = json!({"action":"library"});
    let key = crate::hash(
        alphaxiv::request(&args)
            .unwrap()
            .start()
            .to_string()
            .as_bytes(),
    );
    saved.context["paperCalls"] = json!({key:{"status":"pending","tool":"list_library"},"search1":{"status":"pending","tool":"discover_papers"},"search2":{"status":"complete","tool":"discover_papers"}});
    state::persist(&app, &scope, "test", &saved).unwrap();
    let mut saved = state::load_or_freeze(
        &app,
        &scope,
        "test",
        saved.input.clone(),
        Default::default(),
    )
    .unwrap();
    assert_eq!(
        execute(&app, &scope, "test", &mut saved, &args)
            .await
            .unwrap_err()
            .code,
        "upstream"
    );
    assert_eq!(
        execute(
            &app,
            &scope,
            "test",
            &mut saved,
            &json!({"action":"discover","keywords":["GRPO"],"question":"New GRPO work"})
        )
        .await
        .unwrap_err()
        .code,
        "rate_limited"
    );
    drop(app);
    std::fs::remove_dir_all(dir).unwrap();
}
#[tokio::test]
async fn cached_original_text_pages_without_another_call_and_retains_known_publication_date() {
    let (app, scope, mut saved, dir) = fixture();
    let args =
        json!({"action":"read","url":"https://www.alphaxiv.org/abs/2601.12345","offset":18000});
    let key = crate::hash(
        alphaxiv::request(&args)
            .unwrap()
            .start()
            .to_string()
            .as_bytes(),
    );
    let text = format!("{}{}", "A".repeat(18000), "B".repeat(1200));
    // A full inventory must still permit reading an admitted paper and retain its known date.
    let mut inventory: Vec<_> = (0..251).map(material).collect();
    inventory[0].url = "https://arxiv.org/abs/2601.12345".into();
    let original_id = inventory[0].evidence()["id"].clone();
    saved.set_materials(&inventory);
    saved.context["paperCalls"] = json!({key:{"status":"complete","tool":"get_paper_content","value":{"url":"https://arxiv.org/pdf/2601.12345","text":text,"fulltext":true,"truncated":false}}});
    let response = execute(&app, &scope, "test", &mut saved, &args)
        .await
        .unwrap();
    assert_eq!(response["text"], "B".repeat(1200));
    // Keep the existing landing URL/source ID; its conservative evidence label is unchanged.
    assert_eq!(response["retrievalKind"], "metadata_or_abstract");
    assert_eq!(response["publishedAt"], "2026-01-10");
    assert_eq!(
        response["publicationDateSource"],
        "search_reported_publication"
    );
    assert_eq!(response["sourceId"], original_id);
    assert!(response["nextOffset"].is_null());
    assert_eq!(saved.materials()[0].text.len(), 19200);
    assert!(saved.materials()[0].fulltext);
    assert_eq!(saved.materials().len(), 251);
    drop(app);
    std::fs::remove_dir_all(dir).unwrap();
}
#[tokio::test]
async fn paper_discovery_stays_in_candidate_pool_until_selected() {
    let (app, scope, mut saved, dir) = fixture();
    saved.set_materials(&(0..80).map(material).collect::<Vec<_>>());
    let args = json!({"action":"library"});
    let key = crate::hash(
        alphaxiv::request(&args)
            .unwrap()
            .start()
            .to_string()
            .as_bytes(),
    );
    saved.context["paperCalls"] = json!({key:{"status":"complete","tool":"list_library","value":{"sources":[
        {"url":"https://arxiv.org/abs/2601.12376","title":"A discovery","publishedAt":"2026-01-11"},
        {"url":"https://arxiv.org/abs/2601.12377","title":"Another discovery"}
    ]}}});
    let response = execute(&app, &scope, "test", &mut saved, &args)
        .await
        .unwrap();
    assert_eq!(response["totalCandidates"], 2);
    assert_eq!(response["sources"].as_array().unwrap().len(), 2);
    assert_eq!(saved.materials().len(), 80);
    assert!(response["sources"][0]["candidateOnly"].as_bool().unwrap());
    drop(app);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn dispatch_fence_survives_context_cleanup_and_cannot_replay_or_exceed_budget() {
    let (app, scope, saved, dir) = fixture();
    claim(&app, &scope, "test", "call-one", "discover_papers", 2).unwrap();
    app.store
        .conn()
        .unwrap()
        .execute("DELETE FROM records WHERE kind='agent_run'", [])
        .unwrap();
    assert_eq!(
        claim(&app, &scope, "test", "call-one", "discover_papers", 2)
            .unwrap_err()
            .code,
        "upstream"
    );
    claim(&app, &scope, "test", "call-two", "discover_papers", 2).unwrap();
    assert_eq!(
        claim(&app, &scope, "test", "call-three", "discover_papers", 2)
            .unwrap_err()
            .code,
        "rate_limited"
    );
    let raw: String = app
        .store
        .conn()
        .unwrap()
        .query_row(
            "SELECT data FROM records WHERE kind='paper_receipt' LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(!raw.contains("fake-private-secret"));
    assert!(!raw.contains("query"));
    drop(saved);
    drop(app);
    std::fs::remove_dir_all(dir).unwrap();
}
