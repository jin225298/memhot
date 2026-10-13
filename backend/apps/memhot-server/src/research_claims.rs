use crate::discovery::Material;
use serde_json::{json, Value};

#[derive(Clone)]
pub(super) struct Claim {
    pub text: String,
    pub quote: String,
    pub evidence: usize,
    pub stance: String,
    pub inference: bool,
}

// The same gate owns publication and coverage. Quote matching alone does not
// make a locally proposed experiment or a meeting attribution rule source evidence.
pub(super) fn admissible(check: &Value) -> bool {
    [
        "supported",
        "relevant",
        "scopePreserved",
        "attributionPreserved",
        "sourceBound",
    ]
    .iter()
    .all(|key| check[*key] == true)
}
pub(super) fn grounded_claims(
    output: &Value,
    materials: &[Material],
    evidence: &[Value],
) -> Vec<Claim> {
    let mut out = Vec::new();
    if let Some(claims) = output["claims"].as_array() {
        for claim in claims.iter().take(12) {
            let Some(text) = claim["text"]
                .as_str()
                .filter(|s| !s.trim().is_empty() && s.len() <= 2400)
            else {
                continue;
            };
            let Some(quote) = claim["quote"]
                .as_str()
                .filter(|s| s.trim().len() >= 8 && s.len() <= 2400)
            else {
                continue;
            };
            let Some(key) = claim["evidenceId"].as_str() else {
                continue;
            };
            let Some(index) = evidence.iter().position(|e| e["id"] == key) else {
                continue;
            };
            if materials[index].is_search_snippet() || !materials[index].text.contains(quote) {
                continue;
            }
            let stance = claim["stance"]
                .as_str()
                .filter(|s| matches!(*s, "support" | "counter" | "context"))
                .unwrap_or("context")
                .to_owned();
            out.push(Claim {
                text: text.into(),
                quote: quote.into(),
                evidence: index,
                stance,
                inference: claim["type"] != "fact",
            });
        }
    }
    out
}

pub(super) fn reviewed_claims(
    claims: Vec<Claim>,
    evidence: &mut [Value],
    verdict: &Value,
) -> Vec<Claim> {
    let checks = verdict["checks"].as_array();
    let accepted: Vec<Claim> = claims
        .into_iter()
        .enumerate()
        .filter_map(|(i, mut claim)| {
            let check = checks?
                .iter()
                .find(|v| v["index"].as_u64() == Some(i as u64))?;
            if !admissible(check) {
                return None;
            }
            // Scope limits and missing evidence are useful context, not observed opposing results.
            if claim.stance == "counter" && (claim.inference || check["counterEvidence"] != true) {
                claim.stance = "context".into();
            }
            Some(claim)
        })
        .collect();
    let accepted = distinct_claims(accepted);
    for (index, source) in evidence.iter_mut().enumerate() {
        let claims: Vec<&Claim> = accepted.iter().filter(|c| c.evidence == index).collect();
        if claims.is_empty() {
            continue;
        }
        let mut quotes = Vec::new();
        for claim in &claims {
            if !quotes.contains(&claim.quote.as_str()) {
                quotes.push(claim.quote.as_str());
            }
        }
        source["excerpt"] = json!(quotes.join("\n\n"));
        source["stance"] = json!(if claims.iter().all(|c| c.stance == claims[0].stance) {
            claims[0].stance.as_str()
        } else {
            "context"
        });
        source["citations"] = json!(claims
            .iter()
            .map(|c| json!({
                "claim": c.text, "quote": c.quote, "stance": c.stance,
                "type": if c.inference { "inference" } else { "fact" }
            }))
            .collect::<Vec<_>>());
        source["verification"] = json!("quote_matched_and_support_reviewed");
    }
    accepted
}

fn distinct_claims(claims: impl IntoIterator<Item = Claim>) -> Vec<Claim> {
    let mut distinct: Vec<Claim> = Vec::new();
    for claim in claims {
        if let Some(existing) = distinct.iter_mut().find(|existing| {
            existing.evidence == claim.evidence && existing.quote.trim() == claim.quote.trim()
        }) {
            // Rephrasing one passage is not new evidence. Prefer the existing
            // cautiously labelled statement, without rewriting text or inventing
            // attribution. Equal labels keep the earlier (usually reviewed) item.
            let caution = |c: &Claim| (c.inference, c.stance == "context");
            if caution(&claim) > caution(existing) {
                *existing = claim;
            }
        } else {
            distinct.push(claim);
        }
    }
    distinct
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn same_source_quote_cannot_count_again_under_new_wording() {
        let claim =
            |text: &str, evidence: usize, quote: &str, inference: bool, stance: &str| Claim {
                text: text.into(),
                quote: quote.into(),
                evidence,
                inference,
                stance: stance.into(),
            };
        let kept = distinct_claims([
            claim(
                "An overbroad fact",
                0,
                "A reviewed passage.",
                false,
                "support",
            ),
            claim(
                "The author cautiously interprets the result",
                0,
                "A reviewed passage.",
                true,
                "support",
            ),
            claim(
                "A limited author interpretation",
                0,
                " A reviewed passage. ",
                true,
                "context",
            ),
            claim(
                "Another wording of that interpretation",
                0,
                "A reviewed passage.",
                true,
                "context",
            ),
            claim(
                "A separate source",
                1,
                "A reviewed passage.",
                false,
                "support",
            ),
            claim(
                "A distinct finding on the same page",
                0,
                "Another reviewed passage.",
                false,
                "support",
            ),
        ]);
        assert_eq!(kept.len(), 3);
        assert_eq!(kept[0].text, "A limited author interpretation");
        assert!(kept[0].inference);
        assert_eq!(kept[0].stance, "context");
        assert_eq!(kept[1].evidence, 1);
        assert_eq!(kept[2].quote, "Another reviewed passage.");
        let reverse = distinct_claims([
            claim(
                "An author interpretation",
                0,
                "A reviewed passage.",
                true,
                "context",
            ),
            claim(
                "A later stronger rephrasing",
                0,
                "A reviewed passage.",
                false,
                "support",
            ),
        ]);
        assert_eq!(reverse.len(), 1);
        assert_eq!(reverse[0].text, "An author interpretation");
    }

    #[test]
    fn local_rules_and_proposals_cannot_become_source_claims() {
        let claims = [
            "原文报告的家庭对照结果",
            "会议身份规则",
            "本报告拟定的实验",
            "缺失归属审核",
        ]
        .map(|text| Claim {
            text: text.into(),
            quote: "An observed study result.".into(),
            evidence: 0,
            stance: "context".into(),
            inference: true,
        })
        .to_vec();
        let checks: Vec<_> = (0..4)
            .map(|index| {
                let mut check = json!({"index":index,"supported":true,"relevant":true,
                "scopePreserved":true,"attributionPreserved":true});
                if index < 3 {
                    check["sourceBound"] = json!(index == 0);
                }
                check
            })
            .collect();
        let mut evidence = vec![json!({"id":"source"})];
        let kept = reviewed_claims(claims, &mut evidence, &json!({"checks":checks}));
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].text, "原文报告的家庭对照结果");
        assert_eq!(evidence[0]["citations"].as_array().unwrap().len(), 1);
        assert!(!evidence[0].to_string().contains("会议身份规则"));
        assert!(!evidence[0].to_string().contains("本报告拟定的实验"));
    }
    #[test]
    fn fabricated_quotes_and_cross_source_ids_are_rejected() {
        let materials = vec![Material {
            title: "A".into(),
            url: "https://example.com".into(),
            text: "The study included 500 participants.".into(),
            publisher: "Lab".into(),
            published: None,
            discovered: None,
            discovered_via: None,
            publication_source: "unknown",
            fulltext: true,
            pdf: None,
        }];
        let evidence = vec![materials[0].evidence()];
        let id = evidence[0]["id"].clone();
        let claims = json!({"claims":[{"text":"valid","quote":"included 500 participants","evidenceId":id,"type":"fact"},{"text":"invented","quote":"included 900 participants","evidenceId":id},{"text":"foreign","quote":"included 500 participants","evidenceId":"foreign"}]});
        let kept = grounded_claims(&claims, &materials, &evidence);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].text, "valid");
    }
    #[test]
    fn matching_search_snippet_cannot_be_published_without_reading_its_source() {
        let mut material = Material {
            title: "Search result".into(),
            url: "https://example.org/paper".into(),
            text: "The experiment included 500 participants.".into(),
            publisher: "Example".into(),
            published: None,
            discovered: None,
            discovered_via: None,
            publication_source: "search_reported_publication",
            fulltext: false,
            pdf: None,
        };
        let evidence = vec![material.evidence()];
        let claims = json!({"claims":[{"text":"The experiment included 500 participants.",
            "quote":"included 500 participants", "evidenceId":evidence[0]["id"], "type":"fact"}]});
        assert!(grounded_claims(&claims, &[material.clone()], &evidence).is_empty());
        material.fulltext = true;
        assert_eq!(grounded_claims(&claims, &[material], &evidence).len(), 1);
    }
    #[test]
    fn reviewed_sources_preserve_each_claim_quote_and_mixed_stances() {
        let claim = |text: &str, quote: &str, stance: &str| Claim {
            text: text.into(),
            quote: quote.into(),
            evidence: 0,
            stance: stance.into(),
            inference: false,
        };
        let mut evidence = vec![json!({"id":"one","excerpt":"raw excerpt","stance":"context"})];
        let verdict = json!({"checks":[
            {"index":0,"supported":true,"relevant":true,"scopePreserved":true,"attributionPreserved":true,"sourceBound":true},
            {"index":1,"supported":true,"relevant":true,"scopePreserved":true,"attributionPreserved":true,"sourceBound":true,"counterEvidence":true},
            {"index":2,"supported":false,"relevant":true}
        ]});
        let kept = reviewed_claims(
            vec![
                claim(
                    "silence can trigger text",
                    "Text occurred in 12 silent clips.",
                    "support",
                ),
                claim(
                    "a condition eliminates errors",
                    "No text occurred after gating silence.",
                    "counter",
                ),
                claim("unsupported", "Do not retain this quote.", "support"),
            ],
            &mut evidence,
            &verdict,
        );
        assert_eq!(kept.len(), 2);
        assert_eq!(
            evidence[0]["excerpt"],
            "Text occurred in 12 silent clips.\n\nNo text occurred after gating silence."
        );
        assert_eq!(evidence[0]["stance"], "context");
        assert_eq!(evidence[0]["citations"].as_array().unwrap().len(), 2);
        assert_eq!(
            evidence[0]["citations"][0]["claim"],
            "silence can trigger text"
        );
        assert_eq!(evidence[0]["citations"][1]["stance"], "counter");
        assert!(!evidence[0].to_string().contains("Do not retain"));
    }
    #[test]
    fn limitations_and_unconfirmed_opposition_cannot_satisfy_counter_requirement() {
        let mut evidence = vec![json!({"id":"one"})];
        let claims = [true, false, false]
            .into_iter()
            .enumerate()
            .map(|(i, inference)| Claim {
                text: "scope limitation".into(),
                quote: format!("Only condition {i} was tested."),
                evidence: 0,
                stance: "counter".into(),
                inference,
            })
            .collect();
        let kept = reviewed_claims(
            claims,
            &mut evidence,
            &json!({"checks":[
                {"index":0,"supported":true,"relevant":true,"scopePreserved":true,"attributionPreserved":true,"sourceBound":true,"counterEvidence":true},
                {"index":1,"supported":true,"relevant":true,"scopePreserved":true,"attributionPreserved":true,"sourceBound":true},
                {"index":2,"supported":true,"relevant":true,"scopePreserved":true,"attributionPreserved":true,"sourceBound":true,"counterEvidence":true}
            ]}),
        );
        assert_eq!(kept[0].stance, "context");
        assert_eq!(kept[1].stance, "context");
        assert_eq!(kept[2].stance, "counter");
    }
}
