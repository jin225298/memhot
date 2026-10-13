use super::*;
fn material(n: usize, official: bool) -> Material {
    Material {
        title: format!("Report {n}"),
        url: format!(
            "https://{}/article-{n}",
            if official {
                "blogs.nvidia.com"
            } else {
                "example.org"
            }
        ),
        text: "A specific change with clear limits.".into(),
        published: Some(crate::now()),
        discovered: None,
        discovered_via: Some(
            if official {
                "https://blogs.nvidia.com/feed/"
            } else {
                "https://example.org/feed"
            }
            .into(),
        ),
        publication_source: "feed_publication",
        publisher: if official { "NVIDIA" } else { "Example" }.into(),
        fulltext: true,
        pdf: None,
    }
}
fn story(m: &Material, a: u64, b: u64) -> Value {
    json!({"id":m.title,"title":m.title,"summary":"只有测试版可用，正式发布时间未知。","category":"产品",
        "evidenceIds":[m.evidence()["id"]],"reportingKind":"development",
        "selection":{"scores":[a,b],"average":(a+b)/2}})
}
#[test]
fn selection_requires_two_complete_independent_score_maps() {
    let items = vec![json!({"id":"a"}), json!({"id":"b"})];
    for scores_json in [
        json!([]),
        json!([{"id":"a","score":100}]),
        json!([{"id":"a","score":100},{"id":"a","score":100}]),
        json!([{"id":"a","score":100},{"id":"b","score":101}]),
        json!([{"id":"a","score":100},{"id":"b","score":70.5}]),
        json!([{"id":"a","score":100},{"id":"unknown","score":80}]),
    ] {
        assert!(scores(&json!({"scores":scores_json}), &items).is_err());
    }
    assert_eq!(
        scores(
            &json!({"scores":[{"id":"a","score":61},{"id":"b","score":82}]}),
            &items
        )
        .unwrap()["b"],
        82
    );
}
#[test]
fn tier_thresholds_use_the_sum_and_never_promote_rejected_items_into_overview() {
    let official = material(1, true);
    let media = material(2, false);
    let (main, flashes) = arrange(
        vec![story(&official, 59, 61), story(&media, 75, 76)],
        &[official, media],
        &HashSet::new(),
    );
    assert_eq!(main.len(), 1);
    assert!(flashes.is_empty());
    assert_eq!(main[0]["selection"]["threshold"], 60);
    let overview = layout(&main);
    assert_eq!(overview["overview"], main[0]["summary"]);
    assert_eq!(overview["overviewMode"], "rule_compiled");
    assert!(!overview.to_string().contains("Report 2"));
}
#[test]
fn sources_are_balanced_and_prior_urls_are_not_republished() {
    let materials: Vec<_> = (0..5).map(|n| material(n, true)).collect();
    let items = materials.iter().map(|m| story(m, 80, 80)).collect();
    let seen = HashSet::from([crate::freshness::canonical(&materials[0].url)]);
    let (main, flashes) = arrange(items, &materials, &seen);
    assert_eq!(main.len(), 2);
    assert_eq!(flashes.len(), 2);
    assert!(main.iter().chain(&flashes).all(|s| s["id"] != "Report 0"));
    let overview = layout(&main);
    assert_eq!(overview["leadStoryId"], main[0]["id"]);
    assert_eq!(overview["highlights"], json!([main[1]["id"]]));
}
#[test]
fn claimed_official_identity_cannot_change_source_tier() {
    let mut media = material(0, false);
    media.discovered_via = Some("https://blogs.nvidia.com/feed/".into());
    let (main, _) = arrange(vec![story(&media, 70, 70)], &[media], &HashSet::new());
    assert!(main.is_empty());
}
