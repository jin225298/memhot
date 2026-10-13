use super::*;
fn private(user: &str) -> Scope {
    Scope::Private {
        user: user.into(),
        job: None,
        epoch: 0,
        run_id: "fixture".into(),
        references: vec![],
    }
}
#[test]
fn credentials_are_owner_only_and_never_in_tool_definitions() {
    let config = Config {
        user_id: "owner".into(),
        api_key: "private-test-secret".into(),
    };
    assert!(config.permits(&private("owner")));
    assert!(!config.permits(&private("other")));
    assert!(!config.permits(&Scope::Public {
        edition_key: "e".into(),
        catalog_id: "ai".into()
    }));
    assert!(!definition().to_string().contains(&config.api_key));
}
#[test]
fn requests_force_raw_papers_and_reject_mutations_and_untrusted_urls() {
    let r = request(
        &json!({"action":"read","url":"https://www.alphaxiv.org/abs/2601.12345v2","offset":18000}),
    )
    .unwrap();
    assert_eq!(
        r.args,
        json!({"url":"https://arxiv.org/pdf/2601.12345v2","fullText":true})
    );
    for bad in [
        json!({"action":"delete_folder"}),
        json!({"action":"read","url":"https://arxiv.org/abs/2601.12345","fullText":false}),
        json!({"action":"library","userId":"other"}),
    ] {
        assert!(request(&bad).is_err())
    }
    for url in [
        "https://arxiv.org.evil/abs/2601.12345",
        "https://arxiv.org/abs/2601.12345?secret=x",
        "https://www.alphaxiv.org/settings/api-keys",
        "https://arxiv.org/pdf/2601.12345v0",
    ] {
        assert!(paper_url(url).is_err())
    }
    assert_eq!(
        paper_url("https://arxiv.org/pdf/hep-th/9901001.pdf").unwrap(),
        "https://arxiv.org/pdf/hep-th/9901001"
    );
}
#[test]
fn topic_queries_preserve_keywords_and_validate_dates() {
    let r=request(&json!({"action":"discover","keywords":["GRPO"],"question":"Recent GRPO evaluations","prioritize":"recency"})).unwrap();
    assert_eq!(r.args["keywords"], json!(["GRPO"]));
    assert_eq!(r.args["difficulty"], 2);
    assert!(r.args.get("published_after").is_none());
    for patch in [
        json!({"question":"me@example.com"}),
        json!({"publishedAfter":"2026-02-30"}),
        json!({"publishedAfter":"2026-10-12","publishedBefore":"2026-10-10"}),
        json!({"keywords":[]}),
    ] {
        let mut args =
            json!({"action":"discover","keywords":["GRPO"],"question":"Research on GRPO"});
        for (key, value) in patch.as_object().unwrap() {
            args[key] = value.clone()
        }
        assert!(request(&args).is_err());
    }
}
#[test]
fn broker_denies_child_authorization_and_tools_outside_the_admitted_call() {
    let r = request(&json!({"action":"library"})).unwrap();
    let server = Server {
        endpoint: ENDPOINT,
        client: "memhot-alphaxiv",
        script: "alphaxiv-main.mjs",
        token: Some("private-test-secret"),
        seconds: 60,
        body_limit: 2097152,
    };
    let mut frame = json!({"url":ENDPOINT,"method":"POST","headers":{},"body":json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"list_library","arguments":{"include_papers":true}}}).to_string()});
    assert!(crate::mcp_transport::validate_http(&r, &server, &frame).is_ok());
    assert!(!r.start().to_string().contains("private-test-secret"));
    frame["headers"]["authorization"] = json!("Bearer something");
    assert!(crate::mcp_transport::validate_http(&r, &server, &frame).is_err());
    frame["headers"] = json!({});
    frame["body"]=json!(json!({"jsonrpc":"2.0","method":"tools/call","params":{"name":"delete_folder","arguments":{}}}).to_string());
    assert!(crate::mcp_transport::validate_http(&r, &server, &frame).is_err());
}
#[test]
fn insecure_config_permissions_and_symlinks_are_rejected() {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!("memhot-alpha-config-{}", crate::id()));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("config.json");
    std::fs::write(
        &path,
        r#"{"userId":"owner","apiKey":"private-test-secret"}"#,
    )
    .unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(Config::read(&path).is_err());
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(Config::read(&path).is_ok());
    std::os::unix::fs::symlink(&path, dir.join("link")).unwrap();
    assert!(Config::read(&dir.join("link")).is_err());
    std::fs::remove_dir_all(dir).unwrap();
}
