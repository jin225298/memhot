use crate::{clip, discovery::Material};
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashSet};
fn clean(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('[', "\\[")
        .replace(']', "\\]")
        .replace('#', "\\#")
}
pub(super) fn render(
    topic: &str,
    overview: &Value,
    stories: &[Value],
    flashes: &[Value],
    materials: &[Material],
    steps: usize,
) -> Value {
    let mut body = String::new();
    let mut included = BTreeSet::new();
    for (label, items) in [("重点故事", stories), ("简短动态", flashes)] {
        if items.is_empty() {
            continue;
        }
        body.push_str(&format!("## {label}\n\n"));
        for story in items {
            body.push_str(&format!(
                "### {}\n\n{}\n\n",
                clean(story["title"].as_str().unwrap_or_default()),
                clean(story["summary"].as_str().unwrap_or_default())
            ));
            for (key, label) in [
                ("whyItMatters", "为什么值得关注（编辑解读）"),
                ("limitations", "证据边界"),
                ("readingGuide", "原文阅读角度"),
            ] {
                let t = story[key].as_str().unwrap_or_default();
                if !t.is_empty() {
                    body.push_str(&format!("**{label}**\n\n{}\n\n", clean(t)));
                }
            }
            let mut seen = HashSet::new();
            for fact in story["facts"].as_array().into_iter().flatten() {
                let index = fact["index"].as_u64().unwrap_or(usize::MAX as u64) as usize;
                let Some(m) = materials.get(index) else {
                    continue;
                };
                if !seen.insert(index) {
                    continue;
                }
                included.insert(index);
                body.push_str(&format!(
                    "- [{}]({}) · {}\n",
                    clean(&m.title),
                    m.url.replace(')', "%29"),
                    clean(m.published.as_deref().unwrap_or("发表时间未核实"))
                ));
            }
            body.push('\n');
        }
    }
    let sources: Vec<_> = included.iter().map(|&i| materials[i].evidence()).collect();
    let publisher_count: HashSet<_> = included
        .iter()
        .map(|&i| materials[i].publisher.as_str())
        .filter(|s| !s.is_empty())
        .collect();
    let item_count = stories.len() + flashes.len();
    let minutes = (body.chars().count() + 499) / 500;
    let mut coverage = crate::freshness::coverage_note(&sources, chrono::Utc::now());
    let current_count = stories
        .iter()
        .chain(flashes)
        .filter(|s| s["reportingKind"] != "retrospective")
        .count();
    let mut gaps = Vec::new();
    // One accepted item is useful reading, but does not establish daily coverage.
    if current_count < 2 {
        gaps.push(if current_count == 0 {
            "本期只有历史回顾，尚未核实到新的进展。".to_owned()
        } else {
            "本期仅核实到一条新资料，尚不足以形成完整日报。".to_owned()
        });
    }
    if overview["overviewMode"] == "accepted_summaries" {
        gaps.push("综合总览尚未完成，先展示已核对的内容摘要。".to_owned());
    } else if overview["overviewStatus"] != "ready" {
        gaps.push("总览尚未通过独立核对，已保留通过审核的内容。".to_owned());
    }
    if !gaps.is_empty() {
        coverage = format!("{} {}", gaps.join(" "), coverage);
    }
    let partial = !gaps.is_empty();
    let editorial = json!({"version":"public-editorial-v4","stories":stories,"flashes":flashes,
        "leadStoryId":overview["leadStoryId"].as_str().map(str::to_owned).or_else(||stories.first().or_else(||flashes.first()).and_then(|s|s["id"].as_str()).map(str::to_owned)).unwrap_or_default(),
        "highlights":overview["highlights"].as_array().cloned().unwrap_or_default(),"sections":overview["sections"].as_array().cloned().unwrap_or_default(),"overviewStatus":overview["overviewStatus"],"overviewMode":overview["overviewMode"],
        "metrics":{"storyCount":item_count,"sourceCount":sources.len(),"publisherCount":publisher_count.len(),"readingMinutes":minutes.max(1)},
        "coverageStatus":if current_count<2{"limited"}else{"selected"},"currentItemCount":current_count,"retrospectiveCount":item_count-current_count,"candidateCount":materials.len(),"acceptedStories":item_count,"citationReview":"completed","agentSteps":steps,
        "coverageNote":coverage});
    json!({"title":format!("{} · {}",clip(topic,80),if current_count<2{"阅读摘选"}else{"本期精选"}),"summary":overview["overview"].as_str().unwrap_or_default(),"body":body,"sources":sources,"status":if partial{"partial"}else{"ready"},"gaps":gaps,"editorial":editorial})
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn metrics_and_sources_count_only_accepted_evidence() {
        let m = Material {
            title: "A".into(),
            url: "https://example.com/a".into(),
            text: "Source material".into(),
            published: None,
            discovered: None,
            discovered_via: None,
            publication_source: "page",
            publisher: "Example".into(),
            fulltext: true,
            pdf: None,
        };
        let s = json!({"id":"x","title":"重点","summary":"有限预览","facts":[{"index":0},{"index":0}],"evidenceIds":["a"]});
        let out = render(
            "测试",
            &json!({"overview":"","overviewStatus":"unavailable"}),
            &[s],
            &[],
            &[m],
            2,
        );
        assert_eq!(out["editorial"]["metrics"]["sourceCount"], 1);
        assert_eq!(out["editorial"]["metrics"]["publisherCount"], 1);
        assert_eq!(out["editorial"]["overviewStatus"], "unavailable");
        assert_eq!(out["summary"], "");
        assert_eq!(
            out["body"]
                .as_str()
                .unwrap()
                .matches("https://example.com/a")
                .count(),
            1
        );
    }
}
