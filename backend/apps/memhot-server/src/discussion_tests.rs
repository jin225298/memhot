use super::*;
#[test]
fn discussion_requires_reading_before_citing_search_results() {
    let mut source =
        json!({"id":"a","url":"https://example.org/a","retrievalKind":"search_snippet"});
    let answer = "The study included 500 participants. [Study](https://example.org/a)";
    assert_eq!(
        validate_citations(answer, &[source.clone()])
            .unwrap_err()
            .code,
        "evidence_unread"
    );
    assert!(validate_citations("The evidence is not yet available.", &[source.clone()]).is_ok());
    source["retrievalKind"] = json!("page");
    assert!(validate_citations(answer, &[source]).is_ok());
}
#[test]
fn discussion_lists_only_exact_markdown_or_plain_url_references() {
    let candidates = vec![
        json!({"url":"https://example.org/a"}),
        json!({"url":"https://example.org/b"}),
        json!({"url":"https://example.org/unrelated"}),
    ];
    let answer = "[Paper A](https://example.org/a) and https://example.org/b.\nUseful findings.";
    assert_eq!(referenced_sources(answer, &candidates), candidates[..2]);
    assert!(referenced_sources("No evidence is available.", &candidates).is_empty());
    let mut stored = json!({"text":answer,"sources":candidates});
    filter_sources(&mut stored);
    assert_eq!(stored["sources"].as_array().unwrap().len(), 2);
}
#[test]
fn source_url_prefixes_and_embedded_query_values_are_not_citations() {
    let candidates = vec![json!({"url":"https://example.org/a"})];
    for answer in [
        "https://example.org/another",
        "https://example.org/a?new=1",
        "https://example.org/a.html",
        "https://elsewhere.org/?next=https://example.org/a",
        "[Different resource](https://example.org/a/extra)",
    ] {
        assert!(
            referenced_sources(answer, &candidates).is_empty(),
            "{answer}"
        );
    }
    assert_eq!(
        referenced_sources("来源：<https://example.org/a>。", &candidates),
        candidates
    );
}
#[test]
fn citation_anchors_unknown_urls_queries_and_markdown_punctuation_are_checked() {
    let mut source = json!({"url":"https://example.org/a","retrievalKind":"search_snippet"});
    assert_eq!(
        validate_citations("[来源](https://example.org/a#results)", &[source.clone()])
            .unwrap_err()
            .code,
        "evidence_unread"
    );
    source["retrievalKind"] = json!("page");
    for answer in [
        "[来源](https://example.org/a#results).",
        "来源：<HTTPS://EXAMPLE.org:443/a#results>。",
        "https://example.org/a, followed by discussion.",
    ] {
        assert!(
            validate_citations(answer, &[source.clone()]).is_ok(),
            "{answer}"
        );
        assert_eq!(referenced_sources(answer, &[source.clone()]).len(), 1);
    }
    for answer in [
        "[未知](HTTPS://unknown.example.org/a)",
        "https://example.org/a?token=new#results",
        "https://elsewhere.org/?next=https://example.org/a",
    ] {
        assert_eq!(
            validate_citations(answer, &[source.clone()])
                .unwrap_err()
                .code,
            "evidence_unknown",
            "{answer}"
        );
    }
    let query = json!({"url":"https://example.org/a?version=1","retrievalKind":"page"});
    assert!(validate_citations(
        "[版本](https://example.org/a?version=1#section)",
        &[query.clone()]
    )
    .is_ok());
    assert!(validate_citations("https://example.org/a?version=2", &[query]).is_err());
    let balanced = json!({"url":"https://docs.example.org/method_(type)","retrievalKind":"page"});
    assert!(validate_citations(
        "[方法](https://docs.example.org/method_(type)).",
        &[balanced]
    )
    .is_ok());
}
#[test]
fn cached_snippets_never_become_pages_and_expired_raw_text_is_not_restored() {
    let timestamp = chrono::Utc::now().timestamp();
    let mut evidence = json!({"url":"https://example.org/a","retrievalKind":"search_snippet","excerpt":"Saved excerpt"});
    let mut raw = json!({"url":"https://example.org/a","text":"Cached snippet","createdAt":now(),"expiresAt":timestamp+3600});
    let restored = restored_material(&evidence, Some(&raw), timestamp).unwrap();
    assert!(!restored.fulltext);
    assert!(restored.is_search_snippet());
    raw["fulltext"] = json!(true);
    assert!(
        restored_material(&evidence, Some(&raw), timestamp)
            .unwrap()
            .fulltext
    );
    raw.as_object_mut().unwrap().remove("fulltext");
    evidence["retrievalKind"] = json!("page");
    assert!(
        restored_material(&evidence, Some(&raw), timestamp)
            .unwrap()
            .fulltext
    );
    raw["fulltext"] = json!(false);
    assert!(
        !restored_material(&evidence, Some(&raw), timestamp)
            .unwrap()
            .fulltext
    );
    raw["fulltext"] = json!(true);
    for change in ["expired", "old", "wrong-url"] {
        let mut invalid = raw.clone();
        match change {
            "expired" => invalid["expiresAt"] = json!(timestamp - 1),
            "old" => {
                invalid["createdAt"] =
                    json!((chrono::Utc::now() - chrono::Duration::hours(25)).to_rfc3339())
            }
            _ => invalid["url"] = json!("https://example.org/another"),
        }
        let restored = restored_material(&evidence, Some(&invalid), timestamp).unwrap();
        assert!(!restored.fulltext, "{change}");
        assert_eq!(restored.text, "Saved excerpt", "{change}");
    }
}

#[test]
fn expired_pdf_context_cannot_claim_the_complete_document_is_available() {
    let evidence = json!({"url":"https://example.org/report.pdf","excerpt":"Short cached excerpt",
        "retrievalKind":"page","extraction":{"format":"pdf","pagesRead":3,"totalPages":3,"truncated":false}});
    let restored = restored_material(&evidence, None, chrono::Utc::now().timestamp()).unwrap();
    assert!(!restored.fulltext);
    assert_eq!(restored.text, "Short cached excerpt");
    let pdf = restored.pdf.unwrap();
    assert!(pdf.truncated);
    assert_eq!((pdf.pages_read, pdf.total_pages), (3, 3));
}
