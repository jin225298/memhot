use super::*;
#[test]
fn paid_request_is_fixed_bounded_and_credentials_are_header_only() {
    let query = Query {
        text: "机器人",
        recent: true,
    };
    let (headers, tavily) = request(Paid::Tavily, "secret-test-key", query).unwrap();
    assert!(headers[AUTHORIZATION].is_sensitive());
    assert_eq!(headers[AUTHORIZATION], "Bearer secret-test-key");
    assert!(!tavily.to_string().contains("secret-test-key"));
    assert_eq!(tavily["max_results"], MAX_RESULTS);
    assert_eq!(tavily["search_depth"], "basic");
    assert_eq!(tavily["time_range"], "week");
    assert_eq!(tavily["include_published_date"], true);
    assert_eq!(tavily["include_raw_content"], false);
    let (headers, exa) = request(Paid::Exa, "secret-test-key", query).unwrap();
    assert!(headers["x-api-key"].is_sensitive());
    assert_eq!(exa["contents"]["highlights"], true);
    assert_eq!(exa["numResults"], MAX_RESULTS);
    assert!(exa["startPublishedDate"].is_null());
    assert!(request(Paid::Tavily, "bad\nheader", query).is_err());
    assert!(request(Paid::Exa, " ", query).is_err());
}
#[test]
fn snippets_preserve_provider_and_publication_not_crawl_time() {
    let data = json!({"results":[{"url":"https://example.org/paper","title":"Research","content":"An indexed excerpt","raw_content":"Do not treat as read","published_date":"2026-10-10","last_crawled":"2026-10-13"}]});
    let items = decode(Paid::Tavily, &data, "key").unwrap();
    assert_eq!(
        items[0].published.as_deref(),
        Some("2026-10-10T00:00:00+00:00")
    );
    assert_eq!(
        items[0].discovered_via.as_deref(),
        Some("https://api.tavily.com/search")
    );
    assert_eq!(items[0].publication_source, "search_reported_publication");
    assert_eq!(items[0].text, "An indexed excerpt");
    assert!(!items[0].fulltext);
    let exa = json!({"results":[{"url":"https://example.org/paper","title":"Research","highlights":["One excerpt","Second excerpt"],"publishedDate":"bad date","crawlDate":"2026-10-13","text":"Unrequested full page"}]});
    let items = decode(Paid::Exa, &exa, "key").unwrap();
    assert!(items[0].published.is_none());
    assert!(items[0].discovered.is_none());
    assert_eq!(items[0].text, "One excerpt\nSecond excerpt");
    assert!(!items[0].fulltext);
}
#[test]
fn unsafe_duplicate_and_secret_bearing_results_are_removed() {
    let data = json!({"results":[
        {"url":"http://127.0.0.1/admin","content":"internal"},
        {"url":"https://user:password@example.org/","content":"credential"},
        {"url":"https://example.org/?token=hidden","content":"secret URL"},
        {"url":"https://example.org/leak","content":"secret-test-key"},
        {"url":"https://example.org/good?utm_source=test","content":"valid"},
        {"url":"https://example.org/good","content":"duplicate"}
    ]});
    let items = decode(Paid::Tavily, &data, "secret-test-key").unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].text, "valid");
    assert!(!format!("{items:?}").contains("secret-test-key"));
    let many = json!({"results":(0..30).map(|n|json!({"url":format!("https://example.org/{n}"),"content":"excerpt"})).collect::<Vec<_>>()});
    assert_eq!(decode(Paid::Tavily, &many, "key").unwrap().len(), 30);
}
#[test]
fn empty_malformed_and_failed_paid_search_all_allow_free_fallback() {
    for data in [
        json!({"error":"secret-test-key"}),
        json!({"results":null}),
        json!({"results":[{"url":"https://example.org","content":false}]}),
    ] {
        let result = decode(Paid::Tavily, &data, "secret-test-key");
        assert!(result.is_err());
        let mut output = SearchOutput::default();
        merge(&mut output, "Tavily", result);
        assert!(output.needs_public_fallback);
        assert!(!output.warnings.join("").contains("secret-test-key"));
    }
    let mut output = SearchOutput::default();
    merge(
        &mut output,
        "Exa",
        decode(Paid::Exa, &json!({"results":[]}), "key"),
    );
    assert!(output.needs_public_fallback);
    assert!(output.materials.is_empty());
    merge(
        &mut output,
        "Exa",
        Err(Error::upstream("provider_auth", "secret-test-key")),
    );
    assert!(!output.warnings.join("").contains("secret-test-key"));
}
#[tokio::test]
async fn empty_query_and_absent_key_do_not_make_paid_requests() {
    assert!(!paid_configured(
        &json!({"provider":"public","apiKey":"key"})
    ));
    assert!(!paid_configured(&json!({"provider":"exa","apiKey":" "})));
    assert!(paid_configured(
        &json!({"provider":"tavily","apiKey":"key"})
    ));
    let output = configured_search(&json!({"provider":"tavily","apiKey":"key"}), " ", true).await;
    assert!(output.materials.is_empty());
    assert_eq!(output.warnings, ["搜索问题不能为空"]);
    assert!(paid_search(
        Paid::Exa,
        "",
        Query {
            text: "research",
            recent: false
        }
    )
    .await
    .is_err());
}
#[tokio::test(start_paused = true)]
async fn provider_deadline_cancels_an_unresponsive_call() {
    let start = tokio::time::Instant::now();
    let result = deadline(std::future::pending::<Result<Value>>()).await;
    assert_eq!(result.unwrap_err().code, "timeout");
    assert_eq!(start.elapsed(), REQUEST_TIMEOUT);
}

#[test]
fn interleaving_keeps_every_result_from_both_providers() {
    let result = |prefix: &str| json!({"results": (0..6).map(|n| json!({"url":format!("https://example.com/{prefix}/{n}"),"title":prefix,"content":"Excerpt"})).collect::<Vec<_>>()});
    let mut all = decode(Paid::Tavily, &result("free"), "").unwrap();
    all.extend(decode(Paid::Tavily, &result("paid"), "").unwrap());
    balance(&mut all, 6);
    assert_eq!(all.len(), 12);
    assert_eq!(all.iter().take(8).filter(|m| m.title == "free").count(), 4);
    assert_eq!(all.iter().take(8).filter(|m| m.title == "paid").count(), 4);
}
