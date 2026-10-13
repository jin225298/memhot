#[path = "research_cache.rs"]
mod cache;
#[path = "research_citations.rs"]
pub(super) mod citations;
use crate::research_claims::Claim;
use crate::{
    auth::User,
    clip,
    db::Job,
    discovery::{self, Material},
    error::{Error, Result},
    freshness, hash, id, models, now, App,
};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use cache::report_records;
use citations::Citations;
use serde_json::{json, Value};

pub use crate::research_queue::queue;

pub fn available(report: &Value) -> bool {
    report["availableAt"]
        .as_str()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .is_none_or(|at| at <= chrono::Utc::now())
}
pub async fn get(
    State(app): State<App>,
    User(user): User,
    Path(key): Path<String>,
) -> Result<Json<Value>> {
    let mut report = crate::report_access::get(&app, &user, &key)?;
    report["saved"] = json!(crate::reading::saved(&app, &user, &key)?);
    if crate::reading::incomplete(&report) {
        report["status"] = json!("incomplete");
    }
    Ok(Json(report))
}
pub async fn delete(
    State(app): State<App>,
    User(user): User,
    Path(key): Path<String>,
) -> Result<StatusCode> {
    let mut conn = app.store.conn()?;
    let tx = conn.transaction()?;
    crate::persistence::current(&tx, &user, "report", &key)?.ok_or_else(Error::missing)?;
    crate::persistence::put(
        &tx,
        &user,
        "report_tombstone",
        &json!({"id":key,"createdAt":now()}),
    )?;
    tx.execute(
        "DELETE FROM records WHERE user_id=? AND kind='report' AND id=?",
        rusqlite::params![user, key],
    )?;
    tx.execute(
        "DELETE FROM records WHERE user_id=? AND kind='report_saved' AND id=?",
        rusqlite::params![user, key],
    )?;
    let run_ids: Vec<String> = {
        let mut statement=tx.prepare("SELECT json_extract(data,'$.input.runId') FROM records WHERE user_id=? AND kind='agent_run' AND EXISTS(SELECT 1 FROM json_each(data,'$.context.references') WHERE json_extract(value,'$[0]')='report' AND json_extract(value,'$[1]')=?)")?;
        let values = statement
            .query_map(rusqlite::params![user, key], |r| r.get(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        values
    };
    for run in run_ids {
        for index in 0..8 {
            let operation = hash(format!("{run}:model:{index}").as_bytes());
            tx.execute(
                "UPDATE receipts SET response=NULL WHERE user_id=? AND key=?",
                rusqlite::params![user, operation],
            )?;
        }
    }
    tx.execute("DELETE FROM records WHERE user_id=? AND kind='agent_run' AND EXISTS(SELECT 1 FROM json_each(data,'$.context.references') WHERE json_extract(value,'$[0]')='report' AND json_extract(value,'$[1]')=?)",rusqlite::params![user,key])?;
    tx.execute("DELETE FROM records WHERE user_id=? AND kind IN ('material_translation','material_translation_request') AND json_extract(data,'$.reportId')=?",rusqlite::params![user,key])?;
    tx.execute("DELETE FROM jobs WHERE user_id=? AND kind='material_translation' AND json_extract(payload,'$.reportId')=?",rusqlite::params![user,key])?;
    tx.commit()?;
    Ok(StatusCode::NO_CONTENT)
}
fn escaped(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
pub fn query_safe(text: &str) -> String {
    clip(
        &text
            .split_whitespace()
            .filter(|w| {
                !w.contains('@') && !w.chars().filter(|c| c.is_ascii_digit()).count().ge(&8)
            })
            .collect::<Vec<_>>()
            .join(" "),
        180,
    )
}
/// Discovery currently admits two initial digest searches. Rotate individual topics
/// deterministically rather than merging unrelated interests or starving later ones.
fn digest_queries(profile: &Value, day: i64) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let topics: Vec<_> = profile["interests"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|topic| crate::shared_public::match_topics(&[(*topic).to_owned()]).is_empty())
        .map(query_safe)
        .filter(|q| !q.trim().is_empty() && seen.insert(q.to_lowercase()))
        .collect();
    let start = day.rem_euclid(topics.len().max(1) as i64) as usize * 2 % topics.len().max(1);
    topics
        .iter()
        .cycle()
        .skip(start)
        .take(topics.len().min(2))
        .cloned()
        .collect()
}
fn report_title(kind: &str, topic: &str, thought: Option<&Value>) -> String {
    let label = thought
        .and_then(|t| t["title"].as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(topic);
    format!(
        "{} · {}",
        if kind == "digest" {
            "固定兴趣日报"
        } else {
            "研究材料"
        },
        clip(
            if label.is_empty() {
                "等待你的第一个想法"
            } else {
                label
            },
            40
        )
    )
}
pub async fn run(app: &App, job: &Job) -> Result<Vec<(&'static str, Value)>> {
    let kind = job.payload["kind"].as_str().unwrap_or("research");
    if kind == "digest" && job.payload["digestStage"] != "private" {
        return crate::digest::run(app, job).await;
    }
    crate::billing::ensure_available(app, &job.user, 1)?;
    let epoch = app.store.epoch(&job.user)?;
    let mut profile = app.store.profile(&job.user)?;
    if kind == "digest" {
        let selected = crate::digest::interests(app, &job.user)?;
        if selected.private.is_empty() && selected.feeds.is_empty() {
            return Ok(vec![]);
        }
        profile["interests"] = json!(selected.private);
        profile["feeds"] = json!(selected.feeds);
    } else {
        let cognition = crate::cognition::context(app, &job.user)?;
        let mut interests = profile["interests"].as_array().cloned().unwrap_or_default();
        for item in &cognition {
            if item["type"] == "interest" && !interests.contains(&item["statement"]) {
                interests.push(item["statement"].clone());
            }
        }
        profile["interests"] = json!(interests);
    }
    let thought = if kind == "digest" {
        None
    } else {
        job.payload["thoughtId"]
            .as_str()
            .map(|key| app.store.get(&job.user, "thought", key))
            .transpose()?
    };
    let thought = if kind == "digest" {
        None
    } else if thought.is_some() {
        thought
    } else {
        app.store
            .list(&job.user, "thought")?
            .into_iter()
            .find(|t| matches!(t["status"].as_str(), Some("open" | "kept")))
    };
    let interests = profile["interests"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("、")
        })
        .unwrap_or_default();
    let mut topic = thought
        .as_ref()
        .and_then(|t| t["question"].as_str())
        .filter(|s| !s.trim().is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| interests.clone());
    if let Some(t) = &thought {
        if t["analysisStatus"] != "ready" {
            topic = format!(
                "{}：{}",
                t["title"].as_str().unwrap_or_default(),
                clip(t["quote"].as_str().unwrap_or_default(), 500)
            );
        }
    }
    if let Some(next) = thought.as_ref().and_then(|t| t["nextQuestion"].as_str()) {
        topic.push_str("；继续思考：");
        topic.push_str(&clip(next, 500));
    }
    let topic = if topic.is_empty()
        && kind == "digest"
        && !profile["feeds"].as_array().is_none_or(Vec::is_empty)
    {
        "我的订阅来源新进展".into()
    } else {
        topic
    };
    let mut gaps = Vec::new();
    let (mut materials, warnings) = if topic.is_empty() {
        (
            Vec::new(),
            vec!["尚未提供思考线索或兴趣，请先记录想法或设置兴趣".into()],
        )
    } else if kind == "digest" {
        let date = job.payload["researchDate"]
            .as_str()
            .or_else(|| job.payload["date"].as_str())
            .and_then(|date| chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").ok())
            .unwrap_or_else(|| chrono::Utc::now().date_naive());
        let day = date
            .signed_duration_since(chrono::NaiveDate::from_ymd_opt(1970, 1, 1).unwrap())
            .num_days();
        let queries = digest_queries(&profile, day);
        profile["_discoveryKind"] = json!(kind);
        profile["_initialDigestQueries"] = json!(queries);
        profile["_seenDigestUrls"] =
            json!(freshness::prior_urls(&app.store.list(&job.user, "report")?));
        discovery::discover(&profile, &queries).await
    } else {
        (Vec::new(), Vec::new())
    };
    gaps.extend(warnings);
    if kind == "digest" {
        // A failed/unconfigured editor never publishes raw excerpts as a digest.
        let mut report =
            crate::editorial::compose_private(app, job, epoch, &topic, &materials, &profile)
                .await?;
        report["id"] = json!(id());
        report["kind"] = json!("digest");
        report["question"] = json!(topic);
        report["createdAt"] = json!(now());
        let mut notes = report["gaps"].as_array().cloned().unwrap_or_default();
        notes.extend(gaps.into_iter().map(Value::String));
        report["gaps"] = json!(notes);
        return Ok(report_records(report, job, &materials, epoch));
    }
    let mut evidence: Vec<Value> = materials.iter().map(Material::evidence).collect();
    if !models::configured(&models::config(app, &job.user, "text")?) {
        return Err(Error::upstream(
            "model_unconfigured",
            "文本模型尚未配置，研究尚未开始",
        ));
    }
    let gathered = crate::research_evidence::synthesize(
        app,
        job,
        epoch,
        &topic,
        thought.as_ref(),
        &mut materials,
        &mut evidence,
        &profile,
    )
    .await?;
    let claims = gathered.claims;
    gaps.extend(gathered.gaps);
    if claims.is_empty() {
        return Err(Error::upstream(
            "research_evidence_insufficient",
            "还没有找到能支持这个问题的可靠证据。本次没有生成报告，可稍后重试或补充更具体的问题。",
        ));
    }
    let citations = Citations::new(&claims);
    // Initial thought gaps are questions for research, not its conclusions. The
    // current evidence review owns remaining gaps; appending the old list could
    // say "no usage evidence" after this very run found a real usage study.
    let reasoning = if kind != "digest"
        && gathered.quality["directSources"].as_u64().unwrap_or(0) >= 2
        && gathered.quality["hasPrimaryEvidence"] == true
        && gathered.elapsed < std::time::Duration::from_secs(600)
    {
        match tokio::time::timeout(
            std::time::Duration::from_secs(210),
            crate::research_reasoning::compose(
                app,
                job,
                epoch,
                &topic,
                thought.as_ref(),
                &claims,
                &evidence,
            ),
        )
        .await
        .unwrap_or_else(|_| Err(Error::upstream("timeout", "分析写作超过本次时间预算")))
        {
            Ok(Some(value)) => Some(value),
            Ok(None) => {
                gaps.push("推进想法的分析未通过独立核对，保留已核对证据供继续讨论".into());
                None
            }
            Err(e) if crate::research_evidence::recoverable(&e) => {
                // No replay of an incomplete paid response. Persist only previously
                // audited claims; this report stays partial and records the writing gap.
                gaps.push("深入分析尚未完成，本次先保留已核对的材料。".into());
                None
            }
            Err(e) => return Err(e),
        }
    } else {
        None
    };
    let mut body = if kind == "digest" {
        "近7天发布的新材料，排除近期已推送的来源。\n\n".to_owned()
    } else {
        reasoning
            .as_ref()
            .map(|(_, body)| body.clone())
            .unwrap_or_default()
    };
    if !claims.is_empty() {
        for (label, stance) in [
            ("支持材料", "support"),
            ("相反材料", "counter"),
            ("背景与待验证推断", "context"),
        ] {
            let group: Vec<&Claim> = claims.iter().filter(|c| c.stance == stance).collect();
            if group.is_empty() {
                continue;
            }
            body.push_str(&format!("## {label}\n\n"));
            for c in group {
                body.push_str(&format!(
                    "- {}{} {}\n\n  > {}\n\n",
                    if c.inference {
                        "【推断】"
                    } else {
                        "【来源事实】"
                    },
                    escaped(&c.text),
                    citations.reference(c, &evidence),
                    escaped(&c.quote).replace('\n', " ")
                ));
            }
        }
    }
    if !gaps.is_empty() {
        body.push_str("## 仍需确认\n\n");
    }
    for gap in &gaps {
        body.push_str(&format!("- {}\n", escaped(gap)));
    }
    let ready =
        !claims.is_empty() && (kind == "digest" || (reasoning.is_some() && gathered.sufficient));
    let summary = if let Some((summary, _)) = &reasoning {
        summary.clone()
    } else if let Some(c) = claims.first() {
        format!(
            "目前的材料还不足以回答这个问题。已核对的线索：{}",
            clip(&c.text, 220)
        )
    } else {
        format!("收集到{}条可核对材料；研究结论尚不充分。", evidence.len())
    };
    let published_evidence = citations.sources(&evidence);
    let report = json!({"id":id(),"title":report_title(kind, &topic, thought.as_ref()),"kind":kind,"question":topic,"summary":summary,"body":body,"thoughtId":thought.as_ref().map(|t|&t["id"]),"createdAt":now(),"status":if ready{"ready"}else{"partial"},"sources":published_evidence,"gaps":gaps,"claims":claims.iter().map(|c|json!({"text":c.text,"quote":c.quote,"stance":c.stance,"type":if c.inference{"inference"}else{"fact"},"evidenceIds":[evidence[c.evidence]["id"].clone()]})).collect::<Vec<_>>(),"generator":{"pipeline":"pi-agent-research-v4-source-bound","quality":gathered.quality,"sourceCount":materials.len(),"citationReview":{"status":if claims.is_empty(){"not_completed"}else{"completed"},"acceptedClaims":claims.len()}}});
    Ok(report_records(report, job, &materials, epoch))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn private_digest_queries_are_separate_bounded_and_rotate_without_public_topics() {
        let p = json!({"interests":["AI","商业","产品设计","社会研究","机器人"," 商业 "]});
        assert_eq!(digest_queries(&p, 0), vec!["商业", "产品设计"]);
        assert_eq!(digest_queries(&p, 1), vec!["社会研究", "商业"]);
        let long = "某个独立兴趣".repeat(100);
        let q = digest_queries(&json!({"interests":[long,"另一个方向"]}), 0);
        assert_eq!(q.len(), 2);
        assert!(q[0].chars().count() <= 180);
        assert_eq!(q[1], "另一个方向");
        assert!(!q[0].contains("另一个方向"));
        assert!(digest_queries(
            &json!({"interests":["AI","机器人"],"feeds":["https://example.org/rss"]}),
            0
        )
        .is_empty());
    }
    #[test]
    fn report_title_uses_the_short_thought_label_not_repeated_questions() {
        let thought = json!({"title":"语音识别幻觉的触发条件"});
        assert_eq!(
            report_title("research", &"一组冗长问题？".repeat(20), Some(&thought)),
            "研究材料 · 语音识别幻觉的触发条件"
        );
        assert!(
            report_title(
                "research",
                "topic",
                Some(&json!({"title":"长".repeat(100)}))
            )
            .chars()
            .count()
                <= 80
        );
    }
}
