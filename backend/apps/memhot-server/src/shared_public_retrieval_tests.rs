use super::*;
fn item(url: &str) -> Material {
    Material {
        title: "Robot release".into(),
        url: url.into(),
        text: "search claims this product is verified".repeat(5),
        published: Some(Utc::now().to_rfc3339()),
        discovered: None,
        discovered_via: None,
        publication_source: "search_reported_publication",
        publisher: "Engine".into(),
        fulltext: false,
        pdf: None,
    }
}
#[test]
fn aihot_duplicates_keep_stronger_official_feed_metadata_and_attribution() {
    for reverse in [false, true] {
        let url = "https://openai.com/story";
        let mut discovery = item(url);
        discovery.discovered_via = Some("https://aihot.news/items/fixture".into());
        let mut official = item(url);
        official.publication_source = "feed_publication";
        let discovery = search_candidate(discovery);
        let official = feed_candidate(official, "https://openai.com/news/rss.xml");
        let groups = if reverse {
            vec![vec![official], vec![discovery]]
        } else {
            vec![vec![discovery], vec![official]]
        };
        let mut candidates = candidate_reserve(groups, 1);
        let candidate = candidates.pop().unwrap();
        assert!(candidate.abstract_allowed);
        assert!(!candidate.material.text.is_empty());
        assert_eq!(
            candidate.material.discovered_via.as_deref(),
            Some("https://aihot.news/items/fixture")
        );
        let mut admitted = vec![candidate.material];
        assert_eq!(
            freshness::select(&mut admitted, &HashSet::new(), Utc::now()),
            0
        );
        assert_eq!(admitted.len(), 1);
    }
}
#[test]
fn search_snippets_and_third_party_feed_quotes_are_never_evidence() {
    let searched = search_candidate(item("https://example.org/story"));
    assert!(searched.material.text.is_empty());
    assert!(searched.material.published.is_none());
    let third_party = feed_candidate(
        item("https://other.org/story"),
        "https://openai.com/news/rss.xml",
    );
    assert!(!third_party.abstract_allowed);
    assert!(third_party.material.text.is_empty());
    assert!(third_party.material.published.is_none());
    let official = feed_candidate(
        item("https://openai.com/story"),
        "https://openai.com/news/rss.xml",
    );
    assert!(official.abstract_allowed);
    assert!(!official.material.text.is_empty());
}
#[test]
fn balanced_candidates_do_not_let_a_prolific_feed_fill_the_edition() {
    let groups = ["a.org", "b.org", "c.org", "d.org"]
        .iter()
        .map(|host| {
            (0..10)
                .map(|i| search_candidate(item(&format!("https://{host}/{i}"))))
                .collect()
        })
        .collect();
    let out = balanced(groups, 12);
    assert_eq!(out.len(), 8);
    assert_eq!(
        out.iter()
            .filter(|c| c.material.url.contains("a.org"))
            .count(),
        2
    );
}
#[test]
fn catalog_has_multiple_source_layers_and_filters_unrelated_robotics_items() {
    let topic = &catalog_data::CATALOG[0];
    let layers: HashSet<_> = topic
        .feeds
        .iter()
        .filter_map(|u| catalog_data::policy(u).map(|p| p.layer))
        .collect();
    for layer in ["产品发布", "研究", "开源工具", "行业报道", "论文"] {
        assert!(layers.contains(layer));
    }
    let mut unrelated = item("https://huggingface.co/blog/paint");
    unrelated.title = "Image drawing tutorial".into();
    unrelated.text = "Create pictures".into();
    unrelated.discovered_via = Some("https://huggingface.co/blog/feed.xml".into());
    assert!(!catalog_data::relevant(
        &catalog_data::CATALOG[1],
        &unrelated
    ));
}
#[test]
fn reserve_keeps_replacements_and_search_results_require_topic_relevance() {
    let candidates = (0..8)
        .map(|i| search_candidate(item(&format!("https://a.org/{i}"))))
        .collect();
    assert_eq!(candidate_reserve(vec![candidates], 6).len(), 6);
    let mut unrelated = item("https://arxiv.org/abs/1234.5678");
    unrelated.title = "The Haagerup VOA at c=8".into();
    unrelated.text = "Vertex operator algebras and mathematical representation theory".into();
    unrelated.discovered_via = None;
    assert!(!catalog_data::relevant(
        &catalog_data::CATALOG[0],
        &unrelated
    ));
    unrelated.title = "Language model efficiency improves in a new release".into();
    assert!(catalog_data::relevant(
        &catalog_data::CATALOG[0],
        &unrelated
    ));
}
