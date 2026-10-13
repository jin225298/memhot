use super::*;
use std::collections::HashSet;
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
        let mut candidates = candidate_reserve(groups);
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
    assert!(!searched.material.text.is_empty());
    assert!(searched.material.is_search_snippet());
    assert!(searched.material.published.is_none());
    let third_party = feed_candidate(
        item("https://other.org/story"),
        "https://openai.com/news/rss.xml",
    );
    assert!(!third_party.abstract_allowed);
    assert!(!third_party.material.text.is_empty());
    assert!(third_party.material.is_search_snippet());
    assert!(third_party.material.published.is_none());
    let official = feed_candidate(
        item("https://openai.com/story"),
        "https://openai.com/news/rss.xml",
    );
    assert!(official.abstract_allowed);
    assert!(!official.material.text.is_empty());
}
#[test]
fn reserve_preserves_more_than_forty_eight_from_one_publisher_and_deduplicates() {
    let mut candidates: Vec<_> = (0..251)
        .map(|i| search_candidate(item(&format!("https://a.org/{i}"))))
        .collect();
    candidates.push(search_candidate(item(
        "https://a.org/250?utm_source=duplicate",
    )));
    candidates.push(search_candidate(item("http://127.0.0.1/private")));
    let out = candidate_reserve(vec![candidates]);
    assert_eq!(out.len(), 251);
    assert!(out.iter().all(|c| c.material.url.contains("a.org")));
    assert!(out
        .iter()
        .all(|c| c.material.is_search_snippet() && !c.material.fulltext));
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
    assert_eq!(candidate_reserve(vec![candidates]).len(), 8);
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

#[tokio::test]
async fn prefetch_success_budget_does_not_drop_unread_same_publisher_candidates() {
    let candidates = (0..150)
        .map(|i| search_candidate(item(&format!("https://a.org/{i}"))))
        .collect();
    let (materials, _, _) = prefetch(
        candidates,
        Duration::from_secs(90),
        |candidate| async move {
            let mut m = candidate.material;
            m.fulltext = true;
            m.publication_source = "original_page_metadata";
            m.published = Some(Utc::now().to_rfc3339());
            (m, None, None)
        },
    )
    .await;
    assert_eq!(materials.len(), 150);
    assert_eq!(materials.iter().filter(|m| m.fulltext).count(), 24);
    assert_eq!(
        materials.iter().filter(|m| m.is_search_snippet()).count(),
        126
    );
    assert!(materials
        .iter()
        .filter(|m| !m.fulltext)
        .all(|m| m.published.is_none()));
}
#[tokio::test]
async fn failed_reads_retain_all_candidates_but_never_admit_search_quotes() {
    let candidates = (0..75)
        .map(|i| search_candidate(item(&format!("https://a.org/{i}"))))
        .collect();
    let (mut materials, _, notes) = prefetch(
        candidates,
        Duration::from_secs(90),
        |candidate| async move {
            (
                candidate.material,
                None,
                Some("Original unavailable".into()),
            )
        },
    )
    .await;
    assert_eq!(materials.len(), 75);
    assert_eq!(notes.len(), 75);
    assert!(materials
        .iter()
        .all(|m| m.is_search_snippet() && !m.fulltext));
    assert_eq!(
        freshness::select(&mut materials, &HashSet::new(), Utc::now()),
        75
    );
    assert!(materials.is_empty());
}
#[tokio::test(start_paused = true)]
async fn expired_prefetch_budget_keeps_inflight_and_unstarted_candidates() {
    let candidates = (0..75)
        .map(|i| search_candidate(item(&format!("https://a.org/{i}"))))
        .collect();
    let (materials, _, notes) =
        prefetch(candidates, Duration::from_secs(2), |candidate| async move {
            std::future::pending::<()>().await;
            (candidate.material, None, None)
        })
        .await;
    assert_eq!(materials.len(), 75);
    assert!(materials
        .iter()
        .all(|m| m.is_search_snippet() && !m.fulltext));
    assert_eq!(notes.len(), 1);
}
#[tokio::test]
async fn old_original_read_never_reverts_to_a_newer_discovery_date() {
    let candidates = vec![search_candidate(item("https://a.org/history"))];
    let (mut materials, _, _) = prefetch(
        candidates,
        Duration::from_secs(90),
        |candidate| async move {
            let mut m = candidate.material;
            m.fulltext = true;
            m.published = Some("2020-01-01T00:00:00Z".into());
            m.publication_source = "original_page_metadata";
            (m, None, None)
        },
    )
    .await;
    assert!(materials[0].fulltext);
    assert_eq!(
        materials[0].published.as_deref(),
        Some("2020-01-01T00:00:00Z")
    );
    assert_eq!(materials[0].publication_source, "original_page_metadata");
    assert_eq!(
        freshness::select(&mut materials, &HashSet::new(), Utc::now()),
        1
    );
}
