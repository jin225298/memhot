use super::*;
use serde_json::json;
fn private() -> Scope {
    Scope::Private {
        user: "owner".into(),
        job: None,
        epoch: 0,
        run_id: "run".into(),
        references: vec![],
    }
}
fn public(catalog: &str) -> Scope {
    Scope::Public {
        edition_key: "edition".into(),
        catalog_id: catalog.into(),
    }
}
fn frame(body: Value) -> Value {
    json!({"url":ENDPOINT,"method":"POST","headers":{"content-type":"application/json"},"body":body.to_string()})
}
#[test]
fn public_arguments_are_fixed_and_never_include_user_queries() {
    let request = request(&public("ai"), &json!({"action":"search"}), &json!([])).unwrap();
    assert_eq!(request.args["q"], "artificial intelligence");
    for catalog in ["ai", "robotics", "physics"] {
        assert!(super::request(
            &public(catalog),
            &json!({"action":"search","query":"private user question"}),
            &json!([])
        )
        .is_err());
        assert!(super::request(
            &public(catalog),
            &json!({"action":"daily","period":"2026-10-11"}),
            &json!([])
        )
        .is_err());
    }
    assert!(public_request("physics", "daily").is_err());
    assert!(public_request("robotics", "latest").is_err());
    assert_eq!(
        public_request("robotics", "search").unwrap().args["q"],
        "robotics embodied intelligence"
    );
}
#[test]
fn private_queries_do_not_share_the_fixed_public_request_and_dates_are_checked() {
    let request = request(
        &private(),
        &json!({"action":"search","query":"Claude release"}),
        &json!([]),
    )
    .unwrap();
    assert_eq!(request.args["q"], "Claude release");
    assert_ne!(
        request.start(),
        public_request("ai", "search").unwrap().start()
    );
    for query in [
        "private@example.com",
        "https://internal.example",
        "first\nsecond",
        "x",
    ] {
        assert!(super::request(
            &private(),
            &json!({"action":"search","query":query}),
            &json!([])
        )
        .is_err());
    }
    assert!(super::request(
        &private(),
        &json!({"action":"daily","period":"2026-02-30"}),
        &json!([])
    )
    .is_err());
    assert_eq!(
        super::request(
            &private(),
            &json!({"action":"weekly","period":"2026-W40"}),
            &json!([])
        )
        .unwrap()
        .args["week"],
        "2026-W40"
    );
    assert!(super::request(
        &private(),
        &json!({"action":"search","query":"AI news","period":"2026-10"}),
        &json!([])
    )
    .is_err());
}
#[test]
fn event_ids_must_be_admitted_from_hot_topics_in_the_current_run() {
    let args = json!({"action":"story","storyId":"known-event"});
    assert!(request(&private(), &args, &json!([])).is_err());
    let admitted = json!([{"id":"known-event","url":"https://aihot.news/stories/known-event"}]);
    assert_eq!(
        request(&public("ai"), &args, &admitted).unwrap().tool,
        "aihot_get_story"
    );
    assert!(request(&public("robotics"), &args, &admitted).is_err());
}
#[test]
fn rpc_transport_rejects_other_urls_tools_capabilities_and_headers() {
    let request = public_request("ai", "daily").unwrap();
    let valid = frame(
        json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"aihot_get_daily","arguments":{}}}),
    );
    assert!(transport::validate_http(&request, &Server::aihot(), &valid).is_ok());
    let mut wrong = valid.clone();
    wrong["url"] = json!("https://aihot.news/api/mcp?secret=private");
    assert!(transport::validate_http(&request, &Server::aihot(), &wrong).is_err());
    wrong = valid.clone();
    wrong["headers"]["authorization"] = json!("secret");
    assert!(transport::validate_http(&request, &Server::aihot(), &wrong).is_err());
    for body in [
        json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"aihot_search","arguments":{"q":"private"}}}),
        json!({"jsonrpc":"2.0","id":1,"method":"resources/read","params":{"uri":"file:///etc/passwd"}}),
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{"roots":{}},"clientInfo":{"name":"memhot-aihot","version":"0.3.3"},"protocolVersion":"2025-11-25"}}),
    ] {
        assert!(transport::validate_http(&request, &Server::aihot(), &frame(body)).is_err());
    }
}
#[test]
fn aggregate_summaries_keep_original_urls_and_aihot_provenance_without_becoming_evidence() {
    let value = json!({"sources":[{"title":"AI fixture","url":"https://example.org/original","text":"Synthetic aggregated summary, not original article text.","fulltext":false,"attribution":{"name":"AIHOT","url":"https://aihot.news/items/fixture"}}],"stories":[]});
    let outcome = decode(&value).unwrap();
    let material = &outcome.materials[0];
    assert!(material.is_search_snippet());
    assert!(material.published.is_none());
    assert_eq!(material.evidence()["retrievalKind"], "search_snippet");
    assert_eq!(material.url, "https://example.org/original");
    assert_eq!(
        material.discovered_via.as_deref(),
        Some("https://aihot.news/items/fixture")
    );
    for (field, bad) in [
        ("url", json!("http://127.0.0.1/private")),
        ("fulltext", json!(true)),
        (
            "attribution",
            json!({"name":"AIHOT","url":"https://evil.invalid/"}),
        ),
    ] {
        let mut wrong = value.clone();
        wrong["sources"][0][field] = bad;
        assert!(decode(&wrong).is_err());
    }
}
