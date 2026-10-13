use crate::{clip, discovery::Material, hash};
use serde_json::{json, Value};
use std::collections::HashSet;
pub(super) fn inventory(materials: &[Material], limit: usize) -> Vec<Value> {
    materials.iter().enumerate().map(|(index,m)|{let e=m.evidence();json!({"index":index,"evidenceId":e["id"],"sourceId":e["id"],"title":m.title,"url":m.url,"publisher":m.publisher,"publishedAt":m.published,"retrievalKind":e["retrievalKind"],"extraction":m.extraction(),"text":clip(&m.text,limit),"textLength":m.text.chars().count(),"textTruncated":m.pdf.as_ref().is_some_and(|p|p.truncated) || m.text.chars().count()>limit})}).collect()
}
/// Fairly share a bounded context across candidates. Short abstracts keep their full
/// text; longer originals use spare space, with explicit truncation and source IDs.
pub(super) fn drafting_inventory(materials: &[Material]) -> Vec<Value> {
    let mut low = 0;
    let mut high = 12000;
    while low < high {
        let mid = (low + high + 1) / 2;
        let size: usize = materials
            .iter()
            .map(|m| m.text.chars().count().min(mid))
            .sum();
        if size <= 72000 {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    inventory(materials, low)
}
/// Review the exact evidence that will survive publication, not every discovered page.
pub(super) fn review_inputs(stories: &[Value], materials: &[Material]) -> Vec<Value> {
    let references: usize = stories
        .iter()
        .map(|s| s["evidenceIds"].as_array().map_or(0, Vec::len))
        .sum();
    let limit = (72000 / references.max(1)).min(12000);
    let inventory = inventory(materials, 0);
    stories.iter().map(|story| {
        let ids: HashSet<_> = story["evidenceIds"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
        let mut item = story.clone();
        item["citedMaterials"] = json!(inventory.iter().enumerate().filter_map(|(index, m)| {
            let id = m["evidenceId"].as_str()?;
            if !ids.contains(id) { return None; }
            let mut source = m.clone();
            source["text"] = json!(review_excerpt(&materials[index].text, story, id, limit));
            source["textTruncated"] = json!(materials[index].pdf.as_ref().is_some_and(|p|p.truncated) || materials[index].text.chars().count() > limit);
            source["textLayout"] = json!("Original excerpts; […] marks omitted text. Facts retain all verified quotes.");
            Some(source)
        }).collect::<Vec<_>>());
        item
    }).collect()
}
/// Retain context around late-page citations instead of silently reviewing only a
/// page's introduction. The per-reference budget also bounds repeated sources.
fn review_excerpt(text: &str, story: &Value, source_id: &str, limit: usize) -> String {
    let chars: Vec<_> = text.chars().collect();
    if chars.len() <= limit {
        return text.to_owned();
    }
    let quotes: Vec<_> = story["facts"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|f| f["evidenceId"] == source_id)
        .filter_map(|f| f["quote"].as_str())
        .collect();
    let mut ranges = Vec::new();
    let mut used = 0;
    for quote in quotes {
        let Some(byte_at) = text.find(quote) else {
            continue;
        };
        let start = text[..byte_at].chars().count();
        let end = start + quote.chars().count();
        let range = (start.saturating_sub(180), (end + 180).min(chars.len()));
        if used + range.1 - range.0 + 7 > limit {
            continue;
        }
        used += range.1 - range.0 + 7;
        ranges.push(range);
    }
    if used < limit {
        ranges.push((0, (limit - used).saturating_sub(7)));
    }
    ranges.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (start, end) in ranges {
        if end <= start {
            continue;
        }
        if let Some(last) = merged.last_mut().filter(|last| last.1 >= start) {
            last.1 = last.1.max(end);
        } else {
            merged.push((start, end));
        }
    }
    merged
        .into_iter()
        .map(|(start, end)| chars[start..end].iter().collect::<String>())
        .collect::<Vec<_>>()
        .join("\n[…]\n")
}

fn string(v: &Value, key: &str, max: usize) -> String {
    clip(v[key].as_str().unwrap_or_default().trim(), max)
}
pub(super) fn grounded(draft: &Value, materials: &[Material]) -> (Vec<Value>, Vec<Value>) {
    grounded_at(draft, materials, chrono::Utc::now())
}
fn grounded_at(
    draft: &Value,
    materials: &[Material],
    at: chrono::DateTime<chrono::Utc>,
) -> (Vec<Value>, Vec<Value>) {
    let mut events = HashSet::new();
    let mut used_evidence = HashSet::new();
    let mut extract = |field: &str, limit: usize| -> Vec<Value> {
        draft[field].as_array().into_iter().flatten().filter_map(|story|{
        let mut title=string(story,"title",80);let summary=string(story,"summary",2400);
        let mut indices=HashSet::new();
        let mut seen_quotes=HashSet::new();
        let facts:Vec<Value>=story["facts"].as_array().into_iter().flatten().filter_map(|fact|{
            let index=if let Some(id)=fact["evidenceId"].as_str().or_else(||fact["sourceId"].as_str()){materials.iter().position(|m|m.evidence()["id"]==id)?}else{fact["index"].as_u64()? as usize};
            let m=materials.get(index)?;let quote=string(fact,"quote",800);
            if m.is_search_snippet() || quote.chars().count()<12 || !m.text.contains(&quote) || !seen_quotes.insert((index,quote.clone())){return None;}
            indices.insert(index);
            Some(json!({"index":index,"evidenceId":m.evidence()["id"],"quote":quote}))
        }).take(16).collect();
        if title.is_empty()||summary.is_empty()||facts.is_empty(){return None;}
        // Agents can discover extra sources after initial retrieval. Every published
        // event still needs an actually cited recent original; old baselines may remain.
        let published=indices.iter().filter_map(|&i|materials[i].published.as_deref().and_then(crate::freshness::publication)).filter(|&date|crate::freshness::recent(date,at)).max()?;
        let reporting_kind = string(story,"reportingKind",30);
        if !matches!(reporting_kind.as_str(),"development"|"analysis"|"retrospective") { return None; }
        let limitation = string(story,"limitations",700);
        if limitation.is_empty() || string(story,"whyItMatters",700).is_empty() || string(story,"readingGuide",500).is_empty() { return None; }
        if reporting_kind == "retrospective" && !title.starts_with("回顾") { title=format!("回顾 · {title}"); }
        let event=string(story,"eventKey",160);let event=if event.is_empty(){title.clone()}else{event};
        if !events.insert(event.to_lowercase()) || indices.iter().all(|i|used_evidence.contains(i)){return None;}
        used_evidence.extend(indices.iter().copied());
        let mut ids:Vec<_>=facts.iter().map(|f|f["evidenceId"].clone()).collect();ids.sort_by_key(Value::to_string);ids.dedup();
        let id=format!("story-{}",&hash(format!("{event}:{ids:?}").as_bytes())[..20]);
        let tags:Vec<_>=story["tags"].as_array().into_iter().flatten().filter_map(Value::as_str).filter(|s|!s.trim().is_empty()).take(6).map(|s|clip(s,30)).collect();
        Some(json!({"id":id,"eventKey":event,"title":title,"summary":summary,"category":string(story,"category",30),"whyItMatters":string(story,"whyItMatters",700),"readingGuide":string(story,"readingGuide",500),"facts":facts,"evidenceIds":ids,"coverageCount":indices.len(),"reportingKind":reporting_kind,"limitations":limitation,"publishedAt":published.to_rfc3339(),"tags":tags}))
    }).take(limit).collect()
    };
    let mut stories = extract("stories", 10);
    let flashes = extract("flashes", 4)
        .into_iter()
        .filter_map(|story| {
            if story["reportingKind"] == "retrospective" {
                stories.push(story);
                None
            } else {
                Some(story)
            }
        })
        .collect();
    (stories, flashes)
}
pub(super) fn approved(stories: Vec<Value>, review: &Value) -> Vec<Value> {
    stories
        .into_iter()
        .filter(|story| {
            review["checks"].as_array().into_iter().flatten().any(|c| {
                c["id"] == story["id"]
                    && c["supported"] == true
                    && c["relevant"] == true
                    && c["noUnsupportedDetail"] == true
            })
        })
        .collect()
}
#[cfg(test)]
mod tests {
    use super::*;
    fn material() -> Material {
        Material {
            title: "A".into(),
            url: "https://example.com/story".into(),
            text: "The model is available only in a limited preview.".into(),
            published: Some(chrono::Utc::now().to_rfc3339()),
            discovered: None,
            discovered_via: None,
            publication_source: "page",
            publisher: "Example".into(),
            fulltext: true,
            pdf: None,
        }
    }
    #[test]
    fn short_pdf_excerpts_keep_document_truncation_in_drafting_and_review() {
        let mut m = material();
        m.pdf = Some(crate::discovery::PdfExtraction {
            pages_read: 1,
            total_pages: 9,
            truncated: true,
        });
        let story = json!({"evidenceIds":[m.evidence()["id"]],"facts":[]});
        assert_eq!(drafting_inventory(&[m.clone()])[0]["textTruncated"], true);
        let review = review_inputs(&[story], &[m]);
        assert_eq!(review[0]["citedMaterials"][0]["textTruncated"], true);
        assert_eq!(
            review[0]["citedMaterials"][0]["extraction"]["totalPages"],
            9
        );
    }
    #[test]
    fn fabricated_quotes_and_duplicate_events_are_rejected() {
        let fabricated = json!({"stories":[{"title":"发布","reportingKind":"development","limitations":"仅针对有限预览，不能外推至正式发布。","whyItMatters":"编辑推断：决定是否参与测试。","readingGuide":"核对预览的可用范围。","summary":"摘要","facts":[{"index":0,"quote":"The model is generally available."}]}]});
        assert!(grounded(&fabricated, &[material()]).0.is_empty());
        let valid = json!({"stories":[{"title":"预览","reportingKind":"development","limitations":"仅针对有限预览，不能外推至正式发布。","whyItMatters":"编辑推断：决定是否参与测试。","readingGuide":"核对预览的可用范围。","summary":"有限预览","facts":[{"index":0,"quote":"available only in a limited preview"}]}]});
        let stories = grounded(&valid, &[material()]).0;
        assert_eq!(stories.len(), 1);
        assert!(approved(
            stories.clone(),
            &json!({"checks":[{"id":stories[0]["id"],"supported":true,"relevant":true}]})
        )
        .is_empty());
    }
    #[test]
    fn story_review_excludes_uncited_materials_even_when_they_were_discovered() {
        let cited = material();
        let mut uncited = material();
        uncited.url = "https://example.com/unrelated".into();
        uncited.text = "Uncited details must not authorize a published summary.".into();
        let story = json!({"id":"story","evidenceIds":[cited.evidence()["id"]]});
        let review = review_inputs(&[story], &[cited, uncited]);
        assert_eq!(review[0]["citedMaterials"].as_array().unwrap().len(), 1);
        assert!(!review[0].to_string().contains("Uncited details"));
    }
    #[test]
    fn search_snippets_do_not_become_editorial_evidence() {
        let mut snippet = material();
        snippet.fulltext = false;
        snippet.publication_source = "search_reported_publication";
        let draft = json!({"stories":[{"title":"预览","reportingKind":"development","limitations":"仅针对有限预览，不能外推至正式发布。","whyItMatters":"编辑推断：决定是否参与测试。","readingGuide":"核对预览的可用范围。","summary":"有限预览",
            "facts":[{"index":0,"quote":"available only in a limited preview"}]}]});
        assert!(grounded(&draft, &[snippet]).0.is_empty());
    }
    #[test]
    fn dynamically_discovered_old_unknown_and_future_sources_cannot_be_digest_news() {
        let at = crate::freshness::publication("2026-10-12T12:00:00Z").unwrap();
        for (published, keep) in [
            (Some("2026-09-01T00:00:00Z"), false),
            (None, false),
            (Some("2026-10-13T00:00:00Z"), false),
            (Some("2026-10-05T11:59:59Z"), false),
            (Some("2026-10-05T12:00:00Z"), true),
            (Some("2026-10-12T18:00:00Z"), true),
            (Some("2026-10-12T18:00:01Z"), false),
        ] {
            let mut m = material();
            m.published = published.map(str::to_owned);
            m.discovered = Some(at.to_rfc3339());
            for field in ["stories", "flashes"] {
                let mut draft = json!({});
                draft[field] = json!([{"title":"来源变化","reportingKind":"development","limitations":"仅针对有限预览，不能外推至正式发布。","whyItMatters":"编辑推断：决定是否参与测试。","readingGuide":"核对预览的可用范围。","summary":"实际核对的变化","facts":[{"index":0,"quote":"available only in a limited preview"}]}]);
                let (stories, flashes) = grounded_at(&draft, &[m.clone()], at);
                assert_eq!(
                    !stories.is_empty() || !flashes.is_empty(),
                    keep,
                    "field={field}, publication={published:?}"
                );
            }
        }
    }

    #[test]
    fn old_background_is_kept_only_alongside_an_actually_cited_recent_source() {
        let at = crate::freshness::publication("2026-10-12T12:00:00Z").unwrap();
        let mut old = material();
        old.published = Some("2020-01-01".into());
        let mut new = material();
        new.url = "https://example.com/new".into();
        new.published = Some("Mon, 12 Oct 2026 10:00:00 GMT".into());
        let draft = json!({"stories":[{"title":"与旧结果比较","reportingKind":"development","limitations":"仅针对有限预览，不能外推至正式发布。","whyItMatters":"编辑推断：决定是否参与测试。","readingGuide":"核对预览的可用范围。","summary":"新进展及历史背景","facts":[{"index":0,"quote":"available only in a limited preview"},{"index":1,"quote":"available only in a limited preview"}]}]});
        let (stories, _) = grounded_at(&draft, &[old, new], at);
        assert_eq!(stories.len(), 1);
        assert_eq!(stories[0]["facts"].as_array().unwrap().len(), 2);
        assert_eq!(stories[0]["publishedAt"], "2026-10-12T10:00:00+00:00");
    }
}
