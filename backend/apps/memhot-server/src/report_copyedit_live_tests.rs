//! Explicit paid test of semantic revision against frozen, known-complete drafts.
//! Never repeats the failed upstream collection/review request or touches user data.
use super::*;
use std::{os::unix::fs::PermissionsExt, path::PathBuf};

#[tokio::test]
#[ignore = "explicit isolated paid QA requires provider and frozen sample files"]
async fn revise_known_complete_report_samples() {
    let dir = PathBuf::from(std::env::var("MEMHOT_COPYEDIT_QA_DIR").unwrap());
    let samples = PathBuf::from(std::env::var("MEMHOT_COPYEDIT_QA_INPUT").unwrap());
    assert!(dir.is_absolute() && dir.to_string_lossy().contains("/qa/") && !dir.exists());
    let samples: Value = serde_json::from_slice(&std::fs::read(samples).unwrap()).unwrap();
    let app = App::open(dir.clone()).unwrap();
    let (user, _) = crate::auth::create_pair(&app, "isolated report revision QA", None).unwrap();
    let conn = app.store.conn().unwrap();
    crate::billing::ensure_wallet(&conn, &user).unwrap();
    conn.execute(
        "UPDATE billing_wallets SET included=10 WHERE user_id=?",
        [&user],
    )
    .unwrap();
    drop(conn);
    app.store
        .queue(&user, "research", json!({"kind":"research"}), "copyedit-qa")
        .unwrap();
    let conn = app.store.conn().unwrap();
    let id: String = conn
        .query_row(
            "SELECT id FROM jobs WHERE user_id=? AND dedup='copyedit-qa'",
            [&user],
            |r| r.get(0),
        )
        .unwrap();
    conn.execute(
        "UPDATE jobs SET status='running',attempt=1,lease_until=? WHERE id=?",
        rusqlite::params![chrono::Utc::now().timestamp() + 600, id],
    )
    .unwrap();
    drop(conn);
    let job = crate::db::Job {
        id,
        user,
        kind: "research".into(),
        payload: json!({"kind":"research"}),
        attempt: 1,
    };
    let scope = Scope::private_at(
        &app,
        &job.user,
        Some(&job),
        &job.id,
        app.store.epoch(&job.user).unwrap(),
    )
    .unwrap();
    let write = |name: &str, value: &Value| {
        let file = dir.join(name);
        std::fs::write(&file, serde_json::to_vec_pretty(value).unwrap()).unwrap();
        std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o600)).unwrap();
    };
    let research = if let Ok(path) = std::env::var("MEMHOT_COPYEDIT_QA_PRIOR_RESEARCH") {
        let path = PathBuf::from(path);
        assert!(path.is_absolute() && path.to_string_lossy().contains("/qa/"));
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
    } else {
        reasoning(
            &app,
            &scope,
            &samples["researchInput"],
            &samples["researchDraft"],
        )
        .await
        .unwrap()
    };
    write("research-revision.json", &research);
    let review_scope = Scope::public("copyedit-frozen-public-qa", "ai").unwrap();
    let original = samples["digestStories"].as_array().unwrap();
    let digest = if let Ok(path) = std::env::var("MEMHOT_COPYEDIT_QA_PRIOR_DIGEST") {
        // Explicitly reuse a completed public sample when only the private editing
        // contract changed; no new provider request or reset of an uncertain call.
        let path = PathBuf::from(path);
        assert!(path.is_absolute() && path.to_string_lossy().contains("/qa/"));
        let prior: Vec<Value> = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        for (before, after) in original.iter().zip(&prior) {
            for field in ["id", "facts", "title", "summary", "publishedAt"] {
                assert_eq!(before[field], after[field]);
            }
        }
        prior
    } else {
        digest(&app, &review_scope, original).await.unwrap()
    };
    write("digest-revision.json", &json!(digest));
    assert_eq!(digest.len(), original.len());
    assert_eq!(
        research["sections"].as_array().unwrap().len(),
        samples["researchDraft"]["sections"]
            .as_array()
            .unwrap()
            .len()
    );
    let research_review = agent::text(
        &app,
        &job,
        app.store.epoch(&job.user).unwrap(),
        "qa-final-reasoning-review",
        crate::research_reasoning::REVIEW_INSTRUCTIONS,
        crate::research_reasoning::review_input(&samples["researchInput"], &research),
    )
    .await
    .unwrap();
    let research_review = crate::agent_structured::decode(
        &app,
        &scope,
        "qa-final-reasoning-review",
        &research_review,
        crate::research_reasoning::REVIEW_INSTRUCTIONS,
    )
    .await
    .unwrap();
    write("research-review.json", &research_review);
    let facts = samples["researchInput"]["reviewedClaims"]
        .as_array()
        .unwrap();
    let claims: Vec<_> = facts
        .iter()
        .map(|c| crate::research_claims::Claim {
            text: c["text"].as_str().unwrap().into(),
            quote: c["quote"].as_str().unwrap().into(),
            evidence: c["evidenceIndex"].as_u64().unwrap() as usize,
            stance: c["stance"].as_str().unwrap().into(),
            inference: c["type"] != "fact",
        })
        .collect();
    let mut evidence = vec![Value::Null; claims.iter().map(|c| c.evidence).max().unwrap() + 1];
    for c in facts {
        evidence[c["evidenceIndex"].as_u64().unwrap() as usize] = json!({"id":c["sourceId"],"title":c["source"],"url":c["url"],"publishedAt":c["publishedAt"]});
    }
    let rendered =
        crate::research_reasoning::render(&research, &research_review, &claims, &evidence);
    write("research-rendered.json", &json!(rendered));
    let mut stories = samples["digestReviewStories"].as_array().unwrap().clone();
    for s in &mut stories {
        let revised = digest.iter().find(|d| d["id"] == s["id"]).unwrap();
        for field in ["whyItMatters", "readingGuide", "limitations"] {
            s[field] = revised[field].clone();
        }
    }
    let review = if let Ok(path) = std::env::var("MEMHOT_COPYEDIT_QA_PRIOR_DIGEST_REVIEW") {
        let path = PathBuf::from(path);
        assert!(path.is_absolute() && path.to_string_lossy().contains("/qa/"));
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
    } else {
        let review = agent::run(
            &app,
            &review_scope,
            Spec {
                mode: "review",
                purpose: "qa-final-editorial-review",
                instructions: crate::editorial::REVIEW_INSTRUCTIONS,
                prompt: json!({"topic":"人工智能","asOf":crate::now(),"stories":stories}),
                max_steps: 1,
                max_output_tokens: 8192,
                tools: false,
            },
            ToolContext::default(),
        )
        .await
        .unwrap();
        let review = crate::agent_structured::decode(
            &app,
            &review_scope,
            "qa-final-editorial-review",
            &review.text,
            crate::editorial::REVIEW_INSTRUCTIONS,
        )
        .await
        .unwrap();
        review
    };
    write("digest-review.json", &review);
    let accepted: Vec<_> = digest
        .iter()
        .filter(|s| {
            review["checks"].as_array().into_iter().flatten().any(|c| {
                c["id"] == s["id"]
                    && c["supported"] == true
                    && c["relevant"] == true
                    && c["noUnsupportedDetail"] == true
            })
        })
        .cloned()
        .collect();
    write("digest-accepted.json", &json!(accepted));
    assert!(accepted.len() >= 2);
    let (_, body) = rendered.expect("useful reviewed reasoning remains");
    for unsupported in [
        "最先撞上的阻力是隐私",
        "任何验证都会先卡",
        "一周内被跨场合引用过一次以上",
    ] {
        assert!(!body.contains(unsupported));
    }
    app.store
        .finish(&job, &[], "Isolated semantic revision QA completed")
        .unwrap();
    write(
        "outcome.json",
        &json!({"passed":true,"scope":"frozen completed drafts; semantic revision and independent review, not a fresh retrieval run","digestStories":accepted.len(),"researchSections":body.matches("### ").count(),"manualReviewRequired":true}),
    );
}
