//! Bounded research followed by evidence coverage review and at most one targeted gap pass.
use crate::{
    agent::{self, Scope, Spec, ToolContext},
    clip,
    db::Job,
    discovery::Material,
    error::{Error, Result},
    research_claims::{grounded_claims, reviewed_claims, Claim},
    research_prompts as prompts, research_quality, App,
};
use serde_json::{json, Value};

pub(super) struct Gathered {
    pub claims: Vec<Claim>,
    pub sufficient: bool,
    pub gaps: Vec<String>,
    pub quality: Value,
    pub elapsed: std::time::Duration,
}

// Claims, citation metadata and cached original pages must come from one reviewed pass.
struct ReviewedPass {
    gathered: Gathered,
    materials: Vec<Material>,
    evidence: Vec<Value>,
}
impl ReviewedPass {
    fn restore(
        mut self,
        materials: &mut Vec<Material>,
        evidence: &mut Vec<Value>,
        error: &Error,
        elapsed: std::time::Duration,
    ) -> Gathered {
        *materials = self.materials;
        *evidence = self.evidence;
        self.gathered.quality["supplementError"] = json!(error.code);
        self.gathered
            .gaps
            .push("补充检索未完成，本次保留已核对的材料，尚缺的证据仍未解决。".into());
        self.gathered.elapsed = elapsed;
        self.gathered
    }
}

pub(super) async fn synthesize(
    app: &App,
    job: &Job,
    epoch: i64,
    topic: &str,
    thought: Option<&Value>,
    materials: &mut Vec<Material>,
    evidence: &mut Vec<Value>,
    profile: &Value,
) -> Result<Gathered> {
    let mut scope = Scope::private_at(app, &job.user, Some(job), &job.id, epoch)?;
    if let Some(id) = job.payload["thoughtId"].as_str() {
        scope = scope.with_reference("thought", id);
    }
    let started = std::time::Instant::now();
    let mut previous = Value::Null;
    let mut baseline = Vec::new();
    let mut prior: Option<ReviewedPass> = None;
    // The worker owns an 840s deadline. All collection/review passes together get
    // 600s, leaving a bounded writing/review window and time to commit the result.
    for pass in 0..2 {
        let remaining = std::time::Duration::from_secs(600).saturating_sub(started.elapsed());
        let result = tokio::time::timeout(
            remaining,
            gather_pass(
                app, job, epoch, &scope, topic, thought, materials, evidence, profile, pass,
                &baseline, &previous,
            ),
        )
        .await
        .unwrap_or_else(|_| Err(Error::upstream("timeout", "证据整理超过本次时间预算")));
        let (accepted, assessment, verdict) = match result {
            Ok(value) => value,
            Err(error) if pass > 0 && recoverable(&error) => {
                scope.check(app)?;
                return Ok(prior
                    .expect("first pass is reviewed before a supplementary pass")
                    .restore(materials, evidence, &error, started.elapsed()));
            }
            Err(error) => return Err(error),
        };
        let enough_time = started.elapsed() < std::time::Duration::from_secs(420);
        let mut quality = assessment.value;
        quality["passes"] = json!(pass + 1);
        quality["gapPassSkippedForTime"] =
            json!(!assessment.sufficient && pass == 0 && !enough_time);
        let gathered = Gathered {
            claims: accepted.clone(),
            sufficient: assessment.sufficient,
            gaps: assessment.gaps,
            quality,
            elapsed: started.elapsed(),
        };
        if gathered.sufficient || pass == 1 || !enough_time {
            return Ok(gathered);
        }
        previous = json!({"quality":gathered.quality,"gaps":gathered.gaps,"checks":verdict,
            "acceptedClaims":accepted.iter().map(|c|json!({"text":c.text,"type":if c.inference{"inference"}else{"fact"},
                "evidenceId":evidence[c.evidence]["id"],"quote":c.quote,"stance":c.stance})).collect::<Vec<_>>(),
            "instruction":"这是唯一一次补检机会。保留有用的已核对证据，优先解决缺失子问题和低质量来源；最终返回整合后的claims。"});
        baseline = accepted;
        prior = Some(ReviewedPass {
            gathered,
            materials: materials.clone(),
            evidence: evidence.clone(),
        });
    }
    unreachable!("two bounded passes always return")
}

pub(super) fn recoverable(error: &Error) -> bool {
    matches!(
        error.code,
        "timeout"
            | "busy"
            | "invalid_response"
            | "truncated"
            | "outcome_unknown"
            | "network"
            | "request_failed"
            | "upstream"
    )
}

async fn gather_pass(
    app: &App,
    job: &Job,
    epoch: i64,
    scope: &Scope,
    topic: &str,
    thought: Option<&Value>,
    materials: &mut Vec<Material>,
    evidence: &mut Vec<Value>,
    profile: &Value,
    pass: usize,
    baseline: &[Claim],
    previous: &Value,
) -> Result<(Vec<Claim>, research_quality::Assessment, Value)> {
    let retained_input = claim_input(baseline, evidence);
    let inventory:Vec<_> = materials.iter().map(|m| {
            let e=m.evidence(); json!({"evidenceId":e["id"],"title":m.title,"url":m.url,
                "material":clip(&m.text,if m.fulltext {6000}else{1000}),"extraction":m.extraction(),
                "truncated":m.pdf.as_ref().is_some_and(|p|p.truncated) || m.text.chars().count()>if m.fulltext{6000}else{1000},
                "publishedAt":m.published,"publicationDateSource":m.publication_source,"retrievalKind":e["retrievalKind"]})
        }).collect();
    let output=agent::run(app,&scope,Spec{
            mode:"research",purpose:if pass==0{"research-evidence-v4"}else{"research-gap-evidence-v4"},
            instructions:prompts::GATHER,
            prompt:json!({"question":clip(topic,2400),"asOf":crate::now(),"originalThinking":prompts::thinking(thought),
                "initialMaterials":inventory,"previousReview":previous}),
            max_steps:if pass==0 {6}else{4},max_output_tokens:8192,tools:true,
        },ToolContext{materials:materials.clone(),profile:profile.clone()}).await?;
    *materials = output.materials;
    *evidence = materials.iter().map(Material::evidence).collect();
    let parsed = crate::agent_structured::decode(
        app,
        &scope,
        if pass == 0 {
            "research-evidence-v4"
        } else {
            "research-gap-evidence-v4"
        },
        &output.text,
        prompts::GATHER,
    )
    .await?;
    // Supplementary reads may upgrade a source in place. Recheck exact quotes
    // against the resulting pages instead of trusting stale source indices.
    let claims = candidate_claims(
        grounded_claims(&retained_input, materials, evidence)
            .into_iter()
            .chain(grounded_claims(&parsed, materials, evidence)),
    );
    let checking:Vec<_>=claims.iter().enumerate().map(|(i,c)|json!({"index":i,"claim":c.text,
            "type":if c.inference{"inference"}else{"fact"},"stance":c.stance,"quote":c.quote,
            "sourceTitle":materials[c.evidence].title,"sourceUrl":materials[c.evidence].url,
            "sourcePublishedAt":materials[c.evidence].published,"publicationDateSource":materials[c.evidence].publication_source,
            "extraction":materials[c.evidence].extraction(),"context":quote_context(&materials[c.evidence].text,&c.quote)})).collect();
    let verdict = if claims.is_empty() {
        json!({"answersQuestion":false,"checks":[],"coverage":[],"missing":"未取得读过原文且引文匹配的结论；请先读原始资料，不要无检索直接返回空。"})
    } else {
        let mut input = json!({"question":topic,"originalThinking":prompts::thinking(thought),"claims":checking});
        let purpose = review_purpose(pass, &input);
        input["asOf"] = json!(crate::now());
        let raw = agent::text(app, job, epoch, &purpose, prompts::REVIEW, input).await?;
        crate::agent_structured::decode(app, &scope, &purpose, &raw, prompts::REVIEW).await?
    };
    let assessment = research_quality::assess(&claims, materials, &verdict);
    let accepted = reviewed_claims(claims, evidence, &verdict);
    Ok((accepted, assessment, verdict))
}

fn claim_input(claims: &[Claim], evidence: &[Value]) -> Value {
    json!({"claims":claims.iter().map(|c|json!({"text":c.text,"quote":c.quote,
        "evidenceId":evidence[c.evidence]["id"],"stance":c.stance,
        "type":if c.inference{"inference"}else{"fact"}})).collect::<Vec<_>>()})
}

// Keep competing interpretations until semantic review. A source-derived fact
// must not disappear because a local proposal reuses its quote and is labelled
// more cautiously. There are at most 12 retained + 12 newly gathered candidates.
fn candidate_claims(claims: impl IntoIterator<Item = Claim>) -> Vec<Claim> {
    let mut candidates: Vec<Claim> = Vec::new();
    for claim in claims {
        if !candidates.iter().any(|c| {
            c.evidence == claim.evidence
                && c.quote == claim.quote
                && c.text == claim.text
                && c.inference == claim.inference
                && c.stance == claim.stance
        }) {
            candidates.push(claim);
        }
    }
    candidates
}

fn review_purpose(pass: usize, input: &Value) -> String {
    // A changed/reordered baseline or review contract needs a fresh index-based
    // verdict, including its format repair. Exclude the current wall clock.
    let prefix = if pass == 0 {
        "research-coverage-review-v4"
    } else {
        "research-gap-review-v4"
    };
    format!(
        "{prefix}-{}",
        &crate::hash(
            json!({"contract":prompts::REVIEW,"input":input})
                .to_string()
                .as_bytes()
        )[..32]
    )
}

fn quote_context(text: &str, quote: &str) -> String {
    // Audit the cited passage, not just the beginning of a long page.
    let Some(start) = text.find(quote) else {
        return clip(text, 6000);
    };
    let before = text[..start]
        .chars()
        .rev()
        .take(1800)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    format!("{before}{}", clip(&text[start..], 4200))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn competing_claims_survive_until_source_boundary_review() {
        let fact = Claim {
            text: "Actual N=15 study result".into(),
            quote: "An observed result.".into(),
            evidence: 0,
            stance: "support".into(),
            inference: false,
        };
        let rule = Claim {
            text: "A meeting identity rule".into(),
            inference: true,
            stance: "context".into(),
            ..fact.clone()
        };
        let candidates = candidate_claims([fact.clone(), rule.clone(), fact]);
        assert_eq!(candidates.len(), 2);
        let mut evidence = vec![json!({"id":"source"})];
        let checks = json!({"checks":[
            {"index":0,"supported":true,"relevant":true,"scopePreserved":true,"attributionPreserved":true,"sourceBound":true},
            {"index":1,"supported":true,"relevant":true,"scopePreserved":true,"attributionPreserved":true,"sourceBound":false}]});
        let kept = reviewed_claims(candidates, &mut evidence, &checks);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].text, "Actual N=15 study result");
        assert!(!evidence[0].to_string().contains(&rule.text));
    }

    #[test]
    fn evidence_review_receipts_cannot_replay_against_a_reordered_baseline() {
        let input = json!({"question":"Question","originalThinking":null,"claims":[
            {"index":0,"claim":"A","quote":"Quote A","sourceUrl":"https://example.org/a"},
            {"index":1,"claim":"B","quote":"Quote B","sourceUrl":"https://example.org/b"}]});
        let mut changed = input.clone();
        changed["claims"].as_array_mut().unwrap().swap(0, 1);
        for pass in 0..2 {
            let purpose = review_purpose(pass, &input);
            assert!(purpose.len() <= 80);
            assert_eq!(purpose, review_purpose(pass, &input));
            assert_ne!(purpose, review_purpose(pass, &changed));
            assert_ne!(
                format!("{purpose}-format"),
                format!("{}-format", review_purpose(pass, &changed))
            );
        }
        assert_ne!(review_purpose(0, &input), review_purpose(1, &input));
    }

    #[test]
    fn failed_supplement_restores_claims_metadata_and_original_pages_together() {
        let original = Material {
            title: "Reviewed original".into(),
            url: "https://example.org/original".into(),
            text: "Observed in one limited trial.".into(),
            published: Some("2026-10-01".into()),
            discovered: None,
            discovered_via: None,
            publication_source: "page_metadata",
            publisher: "Original author".into(),
            fulltext: true,
            pdf: None,
        };
        let original_evidence = original.evidence();
        let snapshot = ReviewedPass {
            gathered: Gathered {
                claims: vec![Claim {
                    text: "One limited trial was reported.".into(),
                    quote: original.text.clone(),
                    evidence: 0,
                    stance: "context".into(),
                    inference: false,
                }],
                sufficient: false,
                gaps: vec!["Independent replication is missing.".into()],
                quality: json!({"passes":1}),
                elapsed: std::time::Duration::from_secs(100),
            },
            materials: vec![original.clone()],
            evidence: vec![original_evidence.clone()],
        };
        // A paper read can replace a source in place before supplementary review fails.
        let mut replaced = original;
        replaced.text = "New, unreviewed version without the original quote.".into();
        replaced.published = Some("2026-10-12".into());
        let mut materials = vec![replaced.clone(), replaced];
        let mut evidence: Vec<_> = materials.iter().map(Material::evidence).collect();
        let retained = snapshot.restore(
            &mut materials,
            &mut evidence,
            &Error::upstream("timeout", "test"),
            std::time::Duration::from_secs(600),
        );
        assert_eq!(materials.len(), 1);
        assert_eq!(evidence, vec![original_evidence]);
        let claim = &retained.claims[0];
        assert!(materials[claim.evidence].text.contains(&claim.quote));
        assert_eq!(materials[0].published.as_deref(), Some("2026-10-01"));
        assert_eq!(retained.quality["passes"], 1);
        assert_eq!(retained.quality["supplementError"], "timeout");
        assert_eq!(retained.gaps.len(), 2);
        assert!(!retained.sufficient);
        assert_eq!(retained.elapsed.as_secs(), 600);
        let retained_input = claim_input(&retained.claims, &evidence);
        // Reordered inventory still maps to the same source; replacing that
        // source's page cannot retain a now-absent quotation after a successful pass.
        let unrelated = Material {
            url: "https://example.org/unrelated".into(),
            text: "An unrelated original page.".into(),
            ..materials[0].clone()
        };
        materials.insert(0, unrelated);
        evidence = materials.iter().map(Material::evidence).collect();
        let regrounded = grounded_claims(&retained_input, &materials, &evidence);
        assert_eq!(regrounded.len(), 1);
        assert_eq!(regrounded[0].evidence, 1);
        materials[1].text = "A revised page lacking the first-pass quote.".into();
        assert!(grounded_claims(&retained_input, &materials, &evidence).is_empty());
    }

    #[test]
    fn optional_stage_failure_cannot_bypass_authority_or_billing_errors() {
        for code in [
            "context_deleted",
            "not_found",
            "unauthorized",
            "quota_exhausted",
            "idempotency_conflict",
            "internal",
        ] {
            assert!(!recoverable(&Error::upstream(code, "test")), "{code}");
        }
        assert!(recoverable(&Error::upstream("timeout", "test")));
        assert!(recoverable(&Error::upstream("truncated", "test")));
    }
    #[test]
    fn review_context_preserves_late_quote_and_surrounding_limits() {
        let text = format!(
            "{}LIMITATION: only one site. Measured result. End.",
            "前文".repeat(9000)
        );
        let got = quote_context(&text, "Measured result.");
        assert!(got.contains("LIMITATION: only one site."));
        assert!(got.contains("Measured result."));
        assert!(got.chars().count() <= 6000);
    }
    #[test]
    fn unknown_speaker_remains_unknown_in_both_research_and_writing() {
        let got = prompts::thinking(Some(
            &json!({"summary":"曾经实习","evidenceRefs":[{"speaker":"unknown"}]}),
        ));
        assert_eq!(got["speakerAttribution"], "会议发言人未确认");
    }
}
