//! Convert bounded model proposals to grounded, tenant-owned topic records.
use crate::{
    clip, cognition,
    error::{Error, Result},
    hash, now, topics,
};
use serde_json::{json, Value};
pub struct Batch {
    pub records: Vec<(&'static str, Value)>,
    pub rejections: Vec<Value>,
    pub normalized: usize,
}
/// Isolate invalid proposals: one bad quote cannot discard unrelated grounded topics.
pub fn build(
    output: &Value,
    source: &Value,
    text: &str,
    offset: usize,
    existing: &[Value],
    seed: Option<&Value>,
    tombstones: &[Value],
) -> Result<Batch> {
    let items = output["topics"]
        .as_array()
        .ok_or_else(|| Error::upstream("invalid_response", "模型未返回话题列表"))?;
    if items.len() > 32 {
        return Err(Error::upstream("truncated", "单段话题过多，需要缩小窗口"));
    }
    let mut batch = Batch {
        records: Vec::new(),
        rejections: Vec::new(),
        normalized: 0,
    };
    let mut seed = seed;
    for (kind, items) in [
        ("topic", Some(items)),
        ("cognition", output["cognitions"].as_array()),
    ] {
        for (index, item) in items.into_iter().flatten().enumerate() {
            let Some(matched) = item["quote"]
                .as_str()
                .and_then(|q| crate::formation_quotes::locate(text, q))
            else {
                batch.rejections.push(json!({"kind":kind,"index":index,"reason":"quote_not_in_owned_text","proposal":item}));
                continue;
            };
            let mut grounded = item.clone();
            grounded["quote"] = json!(matched.quote);
            let result = if kind == "topic" {
                build_valid(
                    &json!({"topics":[grounded]}),
                    source,
                    text,
                    offset,
                    existing,
                    seed,
                    tombstones,
                )
            } else {
                let unknown = source["speakerMode"] == "unknown" || source["kind"] == "audio";
                Ok(cognition::proposals(
                    &json!([grounded]),
                    text,
                    &json!({"sourceId":source["id"],"speaker":if unknown{"unknown"}else{"user"}}),
                    !unknown,
                )
                .into_iter()
                .map(|v| ("cognition", v))
                .collect())
            };
            match result {
                Ok(records) if kind == "topic" || !records.is_empty() => {
                    if records.iter().any(|(k, _)| *k == "thought") {
                        seed = None;
                    }
                    batch.normalized += usize::from(matched.normalized);
                    batch.records.extend(records);
                }
                _ => batch.rejections.push(
                    json!({"kind":kind,"index":index,"reason":"invalid_proposal","proposal":item}),
                ),
            }
        }
    }
    Ok(batch)
}
fn build_valid(
    output: &Value,
    source: &Value,
    text: &str,
    offset: usize,
    existing: &[Value],
    seed: Option<&Value>,
    tombstones: &[Value],
) -> Result<Vec<(&'static str, Value)>> {
    let items = output["topics"]
        .as_array()
        .ok_or_else(|| Error::upstream("invalid_response", "模型未返回话题列表"))?;
    if items.len() > 32 {
        return Err(Error::upstream("truncated", "单段话题过多，需要缩小窗口"));
    }
    let key = source["id"].as_str().ok_or_else(Error::internal)?;
    let unknown = source["speakerMode"] == "unknown" || source["kind"] == "audio";
    let mut result = Vec::new();
    let mut seed_used = false;
    for item in items {
        let quote = item["quote"]
            .as_str()
            .filter(|s| !s.trim().is_empty() && s.chars().count() <= 2000)
            .ok_or_else(|| Error::upstream("invalid_response", "模型话题缺少原话"))?;
        let start = crate::formation_quotes::locate(text, quote)
            .ok_or_else(|| Error::upstream("invalid_response", "话题引文不在当前原文窗口"))?
            .byte_start;
        let title = item["title"]
            .as_str()
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| Error::upstream("invalid_response", "话题标题缺失"))?;
        let question = item["question"]
            .as_str()
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| Error::upstream("invalid_response", "话题问题缺失"))?;
        let fingerprint = topics::fingerprint(question);
        let evidence_id =
            hash(format!("{key}:{}:{quote}", offset + text[..start].chars().count()).as_bytes());
        let target = item["existingThoughtId"].as_str().filter(|s| !s.is_empty());
        let owned = if let Some(target) = target {
            Some(
                existing
                    .iter()
                    .find(|t| t["id"] == target)
                    .ok_or_else(|| Error::upstream("invalid_response", "模型引用了未提供的话题"))?,
            )
        } else {
            existing.iter().find(|t| {
                t["fingerprint"] == fingerprint
                    || t["evidenceRefs"]
                        .as_array()
                        .is_some_and(|a| a.iter().any(|e| e["id"] == evidence_id))
            })
        };
        let id = owned
            .and_then(|t| t["id"].as_str())
            .map(str::to_owned)
            .unwrap_or_else(|| {
                format!("topic-{}", hash(format!("{key}:{fingerprint}").as_bytes()))
            });
        if tombstones.iter().any(|t| {
            t["kind"] == "thought"
                && (t["targetId"] == id
                    || t["fingerprint"] == fingerprint
                    || t["fingerprints"]
                        .as_array()
                        .is_some_and(|a| a.contains(&json!(fingerprint)))
                    || t["evidenceIds"]
                        .as_array()
                        .is_some_and(|a| a.contains(&json!(evidence_id))))
        }) {
            continue;
        }
        let mut thought = if let Some(owned) = owned {
            let mut t = owned.clone();
            t["_mustExist"] = json!(true);
            t
        } else if !seed_used && seed.is_some_and(|s| s["analysisStatus"] == "pending") {
            seed_used = true;
            let mut t = seed.cloned().unwrap();
            t["_mustExist"] = json!(true);
            t
        } else {
            json!({"id":id,"status":"open","createdAt":now(),"revision":1,"sourceId":key,"quote":quote,"origin":if unknown{"unverified_transcript"}else{"user_quote"}})
        };
        thought["title"] = json!(clip(title, 160));
        thought["question"] = json!(clip(question, 1500));
        thought["summary"] = json!(clip(item["summary"].as_str().unwrap_or(question), 3000));
        thought["fingerprint"] = json!(fingerprint);
        thought["fingerprints"] = json!([fingerprint]);
        for field in ["assumptions", "gaps"] {
            thought[field] = json!(item[field]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .take(8)
                .map(|s| clip(s, 400))
                .collect::<Vec<_>>());
        }
        let mut refs = thought["sourceIds"].as_array().cloned().unwrap_or_default();
        if let Some(first) = thought["sourceId"].as_str() {
            if !refs.contains(&json!(first)) {
                refs.push(json!(first));
            }
        }
        if !refs.contains(&json!(key)) {
            refs.push(json!(key));
        }
        thought["sourceIds"] = json!(refs);
        let evidence = json!({"id":evidence_id,"sourceId":key,"quote":quote,"summary":thought["summary"],"start":offset+text[..start].chars().count(),"end":offset+text[..start].chars().count()+quote.chars().count(),"speaker":if unknown{"unknown"}else{"user"},"locator":"analyzed_text"});
        let mut evidence_refs = thought["evidenceRefs"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        if !evidence_refs.iter().any(|e| e["id"] == evidence["id"]) {
            evidence_refs.push(evidence.clone());
        }
        thought["evidenceRefs"] = json!(evidence_refs);
        thought["latestQuote"] = evidence;
        thought["analysisStatus"] = json!("ready");
        thought["researchEligible"] = json!(true);
        thought["updatedAt"] = json!(now());
        result.push(("thought", thought));
    }
    result.extend(
        cognition::proposals(
            &output["cognitions"],
            text,
            &json!({"sourceId":key,"speaker":if unknown{"unknown"}else{"user"}}),
            !unknown,
        )
        .into_iter()
        .map(|v| ("cognition", v)),
    );
    Ok(result)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn independent_topics_survive_individual_quote_rejection() {
        let source = json!({"id":"s","kind":"import","speakerMode":"unknown"});
        let input = json!({"topics":[{"title":"A","question":"A?","quote":"第一题"},{"title":"B","question":"B?","quote":"第二题"}],"cognitions":[{"type":"preference","statement":"喜欢第一题","quote":"第一题","origin":"explicit"}]});
        let result = build(&input, &source, "第一题。第二题。", 0, &[], None, &[]).unwrap();
        assert_eq!(
            result
                .records
                .iter()
                .filter(|(kind, _)| *kind == "thought")
                .count(),
            2
        );
        assert_eq!(
            result
                .records
                .iter()
                .find(|(kind, _)| *kind == "cognition")
                .unwrap()
                .1["status"],
            "pending"
        );
        let partial = build(&input, &source, "只有第一题", 0, &[], None, &[]).unwrap();
        assert_eq!(
            partial
                .records
                .iter()
                .filter(|(k, _)| *k == "thought")
                .count(),
            1
        );
        assert_eq!(partial.rejections.len(), 1);
        assert_eq!(partial.rejections[0]["index"], 1);
        assert_eq!(partial.rejections[0]["reason"], "quote_not_in_owned_text");
    }
    #[test]
    fn deleted_evidence_cannot_return_with_a_rephrased_question() {
        let source = json!({"id":"s","speakerMode":"self"});
        let output =
            json!({"topics":[{"title":"变体","question":"换一种问法?","quote":"已有依据"}]});
        let deleted = json!({"kind":"thought","targetId":"old","evidenceIds":[hash("s:0:已有依据".as_bytes())]});
        assert!(
            build(&output, &source, "已有依据", 0, &[], None, &[deleted])
                .unwrap()
                .records
                .is_empty()
        );
    }
    #[test]
    fn same_window_merges_both_evidence_spans_and_normalized_quote_keeps_original() {
        let dir = std::env::temp_dir().join(format!("memhot-evidence-{}", crate::id()));
        let app = crate::App::open(dir.clone()).unwrap();
        let (user, _) = crate::auth::create_pair(&app, "A", None).unwrap();
        let source = json!({"id":"s","speakerMode":"unknown","text":"前文。第一，依据。第二，依据。","createdAt":now()});
        app.store.put(&user, "source", &source, None).unwrap();
        let owned = json!({"id":"owned","sourceId":"s","sourceIds":["s"],"evidenceRefs":[],"revision":1,"createdAt":now()});
        app.store.put(&user, "thought", &owned, None).unwrap();
        let output = json!({"topics":[{"title":"合并","question":"如何研究?","quote":"第一,依据.","existingThoughtId":"owned"},{"title":"合并","question":"如何研究?","quote":"第二，依据。","existingThoughtId":"owned"}]});
        let batch = build(
            &output,
            &source,
            "第一，依据。第二，依据。",
            3,
            &[owned],
            None,
            &[],
        )
        .unwrap();
        assert_eq!(batch.normalized, 1);
        assert!(batch.rejections.is_empty());
        app.store
            .queue(&user, "formation", json!({"sourceId":"s"}), "test")
            .unwrap();
        let job = app.store.claim().unwrap().unwrap();
        assert!(app.store.checkpoint(&job, &batch.records).unwrap());
        let topic = app.store.get(&user, "thought", "owned").unwrap();
        let evidence = topic["evidenceRefs"].as_array().unwrap();
        assert_eq!(evidence.len(), 2);
        assert_eq!(evidence[0]["start"], 3);
        assert_eq!(evidence[0]["quote"], "第一，依据。");
        assert_eq!(evidence[1]["start"], 9);
        assert!(evidence.iter().all(|e| e["speaker"] == "unknown"));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
