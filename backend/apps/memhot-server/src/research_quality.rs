//! Coverage is evaluated after quote/support review, not by output length or counter labels.
use crate::{discovery::Material, research_claims::Claim};
use serde_json::{json, Value};
use std::collections::HashSet;

pub(super) struct Assessment {
    pub sufficient: bool,
    pub gaps: Vec<String>,
    pub value: Value,
}

pub(super) fn assess(claims: &[Claim], materials: &[Material], review: &Value) -> Assessment {
    let checks = review["checks"].as_array().cloned().unwrap_or_default();
    let valid = |i: usize| {
        checks
            .iter()
            .find(|c| c["index"].as_u64() == Some(i as u64))
            .filter(|c| crate::research_claims::admissible(c))
    };
    let mut sources = HashSet::new();
    let mut hosts = HashSet::new();
    let mut groups = HashSet::new();
    let mut primary = false;
    for (i, claim) in claims.iter().enumerate() {
        if let Some(c) = valid(i).filter(|c| c["directAnswer"] == true) {
            if matches!(c["sourceType"].as_str(), Some("primary" | "secondary")) {
                let m = &materials[claim.evidence];
                sources.insert(claim.evidence);
                if let Ok(url) = reqwest::Url::parse(&m.url) {
                    hosts.insert(
                        url.host_str()
                            .unwrap_or_default()
                            .trim_start_matches("www.")
                            .to_owned(),
                    );
                }
                if let Some(group) = c["independenceGroup"]
                    .as_str()
                    .filter(|s| !s.trim().is_empty())
                {
                    groups.insert(group.trim().to_lowercase());
                }
                primary |= c["sourceType"] == "primary";
            }
        }
    }
    let mut gaps = Vec::new();
    let coverage: Vec<_> = review["coverage"]
        .as_array()
        .into_iter()
        .flatten()
        .take(4)
        .map(|item| {
            let grounded = item["claimIndices"].as_array().is_some_and(|indices| {
                !indices.is_empty()
                    && indices.iter().all(|index| {
                        index.as_u64().is_some_and(|n| {
                            (n as usize) < claims.len()
                                && valid(n as usize).is_some_and(|c| {
                                    c["directAnswer"] == true
                                        && matches!(
                                            c["sourceType"].as_str(),
                                            Some("primary" | "secondary")
                                        )
                                })
                        })
                    })
            });
            let answered = item["answered"] == true && grounded;
            let question = crate::clip(item["question"].as_str().unwrap_or("核心问题"), 200);
            let missing = crate::clip(item["missing"].as_str().unwrap_or_default(), 350);
            if !answered {
                gaps.push(if missing.is_empty() {
                    format!("尚未找到直接回答“{question}”的证据")
                } else {
                    missing.clone()
                });
            }
            json!({"question":question,"answered":answered,"missing":missing})
        })
        .collect();
    if sources.len() < 2 || groups.len() < 2 {
        gaps.push("直接相关的独立来源仍不足，暂不能形成交叉验证".into());
    }
    if !primary {
        gaps.push("尚缺直接回答问题的原始研究、数据或当事方披露".into());
    }
    let covered =
        (2..=4).contains(&coverage.len()) && coverage.iter().all(|v| v["answered"] == true);
    if !covered && gaps.is_empty() {
        gaps.push("关键子问题尚未完成证据核对".into());
    }
    if review["answersQuestion"] != true && gaps.is_empty() {
        gaps.push("已有材料仍未直接回答核心问题".into());
    }
    let sufficient = covered
        && review["answersQuestion"] == true
        && sources.len() >= 2
        && groups.len() >= 2
        && primary;
    gaps.dedup();
    Assessment {
        sufficient,
        value: json!({"sufficient":sufficient,"directSources":sources.len(),"sourceHosts":hosts.len(),"independentEvidenceGroups":groups.len(),"hasPrimaryEvidence":primary,"coverage":coverage}),
        gaps,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (Vec<Claim>, Vec<Material>, Value) {
        let materials = [
            "https://lab.example/trial",
            "https://journal.example/review",
        ]
        .map(|url| Material {
            title: "source".into(),
            url: url.into(),
            text: "Actual measured result".into(),
            published: None,
            discovered: None,
            discovered_via: None,
            publication_source: "unknown",
            publisher: "source".into(),
            fulltext: true,
            pdf: None,
        })
        .to_vec();
        let claims = (0..2)
            .map(|evidence| Claim {
                text: "specific result".into(),
                quote: "Actual measured result".into(),
                evidence,
                stance: "context".into(),
                inference: false,
            })
            .collect();
        let review = json!({"answersQuestion":true,"checks":[
            {"index":0,"supported":true,"relevant":true,"scopePreserved":true,"attributionPreserved":true,"sourceBound":true,"directAnswer":true,"sourceType":"primary","independenceGroup":"original-study-a"},
            {"index":1,"supported":true,"relevant":true,"scopePreserved":true,"attributionPreserved":true,"sourceBound":true,"directAnswer":true,"sourceType":"secondary","independenceGroup":"independent-observation-b"}],
            "coverage":[{"question":"效果","answered":true,"claimIndices":[0]},{"question":"限制","answered":true,"claimIndices":[1]}]});
        (claims, materials, review)
    }
    #[test]
    fn opinions_one_source_or_uncovered_questions_cannot_become_deep_research() {
        let (claims, mut materials, review) = fixture();
        assert!(assess(&claims, &materials, &review).sufficient); // No fabricated counterexample required.
        materials[1].url = "https://lab.example/other".into();
        assert!(assess(&claims, &materials, &review).sufficient); // Same publisher may host independent studies.
        let (claims, materials, mut review) = fixture();
        review["checks"][1]["independenceGroup"] = json!("original-study-a");
        assert!(!assess(&claims, &materials, &review).sufficient);
        review["checks"][1]["independenceGroup"] = json!("independent-observation-b");
        for invalid in [Value::Null, json!("primray"), json!("opinion")] {
            review["checks"][0]["sourceType"] = invalid;
            assert!(!assess(&claims, &materials, &review).sufficient);
        }
        review["checks"][0]["sourceType"] = json!("opinion");
        assert!(!assess(&claims, &materials, &review).sufficient);
        review["checks"][0]["sourceType"] = json!("primary");
        review["coverage"][1]["claimIndices"] = json!([99]);
        assert!(!assess(&claims, &materials, &review).sufficient);
    }
    #[test]
    fn out_of_scope_or_misattributed_claims_never_count_towards_coverage() {
        let (claims, materials, review) = fixture();
        for field in [
            "supported",
            "relevant",
            "scopePreserved",
            "attributionPreserved",
            "directAnswer",
            "sourceBound",
        ] {
            let mut bad = review.clone();
            bad["checks"][0][field] = json!(false);
            assert!(!assess(&claims, &materials, &bad).sufficient, "{field}");
        }
    }
}

#[cfg(test)]
#[path = "research_quality_live_tests.rs"]
mod live_tests;
