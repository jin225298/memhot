//! Regressions for evidence lost between editing, review and publication.
use super::{grounding, rendering};
use crate::discovery::Material;
use serde_json::{json, Value};

fn material(text: &str) -> Material {
    Material {
        title: "Interview".into(),
        url: "https://example.org/interview".into(),
        text: text.into(),
        published: Some(crate::now()),
        discovered: None,
        discovered_via: None,
        publication_source: "original_page_metadata",
        publisher: "Example".into(),
        fulltext: true,
        pdf: None,
    }
}
fn story() -> Value {
    json!({"eventKey":"investment-history","title":"2018年的投资回顾",
        "summary":"受访者回顾2018年的投资决策，属于历史材料，无法确认当前融资状态。",
        "whyItMatters":"编辑推断：可用来比较投资人当时考虑的条件，不能作为当前投资依据。",
        "readingGuide":"核对受访者如何描述领投方退出及后续打款的先后关系。",
        "limitations":"单方事后回忆，没有融资文件或其他当事人的交叉确认。",
        "reportingKind":"retrospective",
        "facts":[{"index":0,"quote":"The investment was completed in 2018."},
            {"index":0,"quote":"The lead investor had withdrawn before the transfer."},
            {"index":0,"quote":"The investment was completed in 2018."}]})
}
#[test]
fn distinct_quotes_from_one_source_survive_the_whole_review_boundary() {
    let m = material("The investment was completed in 2018. The lead investor had withdrawn before the transfer.");
    let (stories, _) = grounding::grounded(&json!({"stories":[story()]}), &[m.clone()]);
    assert_eq!(stories.len(), 1);
    assert_eq!(stories[0]["facts"].as_array().unwrap().len(), 2);
    assert_eq!(stories[0]["evidenceIds"].as_array().unwrap().len(), 1);
    assert_eq!(stories[0]["coverageCount"], 1);
    let reviewed = grounding::review_inputs(&stories, &[m]);
    assert!(reviewed[0]["citedMaterials"][0]["text"]
        .as_str()
        .unwrap()
        .contains("lead investor"));
    assert_eq!(reviewed[0]["facts"].as_array().unwrap().len(), 2);
}
#[test]
fn history_cannot_be_published_as_a_complete_daily_or_as_a_flash() {
    let m = material("The investment was completed in 2018. The lead investor had withdrawn before the transfer.");
    let (stories, flashes) = grounding::grounded(&json!({"flashes":[story()]}), &[m.clone()]);
    assert_eq!(stories.len(), 1);
    assert!(flashes.is_empty());
    let report = rendering::render(
        "投资",
        &json!({"overview":"历史访谈摘选。","overviewStatus":"ready","overviewMode":"synthesized"}),
        &stories,
        &flashes,
        &[m],
        3,
    );
    assert_eq!(report["status"], "partial");
    assert_eq!(report["editorial"]["currentItemCount"], 0);
    assert_eq!(report["editorial"]["retrospectiveCount"], 1);
    assert_eq!(report["title"], "投资 · 阅读摘选");
    assert!(report["body"].as_str().unwrap().contains("证据边界"));
    assert!(report["gaps"].to_string().contains("只有历史回顾"));
}
#[test]
fn drafts_missing_usefulness_or_evidence_limits_do_not_pass_grounding() {
    let m = material("The investment was completed in 2018. The lead investor had withdrawn before the transfer.");
    for field in [
        "whyItMatters",
        "readingGuide",
        "limitations",
        "reportingKind",
    ] {
        let mut incomplete = story();
        incomplete.as_object_mut().unwrap().remove(field);
        assert!(
            grounding::grounded(&json!({"stories":[incomplete]}), &[m.clone()])
                .0
                .is_empty(),
            "{field}"
        );
    }
}
#[test]
fn drafting_receives_more_than_leading_1800_chars_and_truncation_is_explicit() {
    let important = format!(
        "{}Results and limitations appear after the introduction.",
        "a".repeat(1900)
    );
    let inventory = grounding::drafting_inventory(&[material(&important)]);
    assert!(inventory[0]["text"]
        .as_str()
        .unwrap()
        .contains("Results and limitations"));
    assert_eq!(inventory[0]["textTruncated"], false);
    let many = vec![material(&"中".repeat(20000)); 40];
    let inventory = grounding::drafting_inventory(&many);
    let chars: usize = inventory
        .iter()
        .map(|m| m["text"].as_str().unwrap().chars().count())
        .sum();
    assert!(chars <= 72000);
    assert!(inventory.iter().all(|m| m["textTruncated"] == true));
    assert!(inventory.iter().all(|m| m["textLength"] == 20000));
    let mixed = grounding::drafting_inventory(&[
        material("Short complete abstract."),
        material(&"x".repeat(15000)),
    ]);
    assert_eq!(mixed[0]["textTruncated"], false);
    assert_eq!(mixed[1]["text"].as_str().unwrap().chars().count(), 12000);
}
#[test]
fn valid_brief_is_still_partial_when_overview_falls_back() {
    let mut a = story();
    a["reportingKind"] = json!("analysis");
    let mut b = a.clone();
    b["id"] = json!("b");
    let report = rendering::render(
        "投资",
        &json!({"overview":"已核对的摘要。","overviewStatus":"ready","overviewMode":"accepted_summaries"}),
        &[a, b],
        &[],
        &[],
        3,
    );
    assert_eq!(report["status"], "partial");
    assert!(report["gaps"].to_string().contains("综合总览尚未完成"));
}

#[test]
fn review_keeps_late_page_quote_context_and_bounds_repeated_sources() {
    let text = format!("{} The investment was completed in 2018. The lead investor had withdrawn before the transfer. These are retrospective claims, not current funding. {}", "前言".repeat(8000), "后文".repeat(3000));
    let m = material(&text);
    let (stories, _) = grounding::grounded(&json!({"stories":[story()]}), &[m.clone()]);
    let reviews = grounding::review_inputs(&stories, &[m.clone()]);
    let excerpt = reviews[0]["citedMaterials"][0]["text"].as_str().unwrap();
    assert!(excerpt.contains("lead investor had withdrawn"));
    assert!(excerpt.contains("retrospective claims, not current funding"));
    assert_eq!(reviews[0]["citedMaterials"][0]["textTruncated"], true);
    let repeated = vec![stories[0].clone(); 14];
    let reviews = grounding::review_inputs(&repeated, &[m]);
    let count: usize = reviews
        .iter()
        .map(|r| {
            r["citedMaterials"][0]["text"]
                .as_str()
                .unwrap()
                .chars()
                .count()
        })
        .sum();
    assert!(count <= 72000);
}
