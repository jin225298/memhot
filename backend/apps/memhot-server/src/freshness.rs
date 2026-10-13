//! Digest recency is measured from publication, never from the time we fetched a page.
use crate::discovery::Material;
use chrono::{DateTime, Duration, NaiveDate, NaiveDateTime, Utc};
use serde_json::Value;
use std::collections::HashSet;
#[path = "freshness_36kr.rs"]
mod kr36;

pub fn publication(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|v| v.with_timezone(&Utc))
        .or_else(|| {
            DateTime::parse_from_rfc2822(value)
                .ok()
                .map(|v| v.with_timezone(&Utc))
        })
        .or_else(|| {
            // Some publishers expose a local wall-clock publication time without an offset.
            // Keep that explicit date; never substitute the fetch time or server timezone.
            NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S")
                .ok()
                .map(|v| v.and_utc())
        })
        .or_else(|| {
            NaiveDate::parse_from_str(value, "%Y-%m-%d")
                .or_else(|_| NaiveDate::parse_from_str(value, "%Y/%m/%d"))
                .ok()
                .and_then(|v| v.and_hms_opt(0, 0, 0))
                .map(|v| v.and_utc())
        })
}

pub fn canonical(url: &str) -> String {
    let Ok(mut url) = reqwest::Url::parse(url) else {
        return url.to_owned();
    };
    url.set_fragment(None);
    let pairs: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(k, _)| !k.starts_with("utm_") && !matches!(k.as_ref(), "fbclid" | "gclid"))
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    url.set_query(None);
    if !pairs.is_empty() {
        url.query_pairs_mut().extend_pairs(pairs);
    }
    url.to_string()
}

pub fn prior_urls(reports: &[Value]) -> HashSet<String> {
    reports
        .iter()
        .filter(|r| r["kind"] == "digest")
        .filter_map(|r| r["sources"].as_array())
        .flatten()
        .filter_map(|s| s["url"].as_str())
        .map(canonical)
        .collect()
}

pub(crate) fn recent(date: DateTime<Utc>, now: DateTime<Utc>) -> bool {
    date >= now - Duration::days(7) && date <= now + Duration::hours(6)
}
/// Keep undated candidates only for the bounded original-page lookup. Final select rejects them.
pub fn prepare(items: &mut Vec<Material>, seen: &HashSet<String>, now: DateTime<Utc>) {
    let mut unique = seen.clone();
    items.retain(|m| {
        m.published
            .as_deref()
            .and_then(publication)
            .is_none_or(|d| recent(d, now))
            && unique.insert(canonical(&m.url))
    });
}
fn json_dates(value: &Value, dates: &mut Vec<DateTime<Utc>>, depth: usize) {
    if depth > 10 {
        return;
    }
    match value {
        Value::Array(items) => {
            for item in items.iter().take(30) {
                json_dates(item, dates, depth + 1)
            }
        }
        Value::Object(map) => {
            let types: Vec<_> = match &value["@type"] {
                Value::String(t) => vec![t.as_str()],
                Value::Array(a) => a.iter().filter_map(Value::as_str).collect(),
                _ => Vec::new(),
            };
            if types
                .iter()
                .any(|t| t.contains("Article") || matches!(*t, "BlogPosting" | "VideoObject"))
            {
                for field in ["datePublished", "uploadDate"] {
                    if let Some(date) = map.get(field).and_then(Value::as_str).and_then(publication)
                    {
                        dates.push(date);
                    }
                }
            }
            for key in ["@graph", "mainEntity"] {
                if let Some(child) = map.get(key) {
                    json_dates(child, dates, depth + 1)
                }
            }
        }
        _ => {}
    }
}
pub(crate) fn requires_article_date(url: &str) -> bool {
    kr36::matches(url)
}
/// Explicit publication metadata only: dateModified, arbitrary time tags and page fetch time are not publication.
pub fn page_publication(url: &str, html: &str) -> Option<String> {
    if requires_article_date(url) {
        return kr36::publication(url, html);
    }
    let doc = dom_query::Document::from(html);
    let mut dates = Vec::new();
    for selector in [
        "meta[property='article:published_time']",
        "meta[itemprop='datePublished']",
        "meta[name='pubdate']",
        "meta[name='publishdate']",
        "meta[name='publication_date']",
        "meta[name='citation_publication_date']",
        // arXiv uses this first-submission field; online/update dates cannot redate it.
        "meta[name='citation_date']",
    ] {
        for meta in doc.select(selector).iter().take(8) {
            if let Some(date) = meta.attr("content").as_deref().and_then(publication) {
                dates.push(date)
            }
        }
    }
    for selector in [
        "time[itemprop='datePublished'][datetime]",
        "time[pubdate][datetime]",
    ] {
        for time in doc.select(selector).iter().take(8) {
            if let Some(date) = time.attr("datetime").as_deref().and_then(publication) {
                dates.push(date)
            }
        }
    }
    for script in doc
        .select("script[type='application/ld+json']")
        .iter()
        .take(12)
    {
        let text = script.text();
        if text.len() > 65536 {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<Value>(&text) {
            json_dates(&value, &mut dates, 0)
        }
    }
    // Conflicting publication metadata is handled conservatively; a newer update cannot redate an old article.
    dates.into_iter().min().map(|d| d.to_rfc3339())
}
pub fn apply_page_date(material: &mut Material, page_url: &str, html: &str) {
    if let Some(date) = page_publication(page_url, html) {
        material.published = Some(date);
        material.publication_source = "original_page_metadata";
    } else if requires_article_date(page_url) {
        material.published = None;
        material.publication_source = "unknown";
    }
}

/// Seven days cover weekends and sparse fields, while per-account history prevents daily repeats.
/// Unknown/future publication dates are excluded rather than described as today's news.
pub fn select(items: &mut Vec<Material>, seen: &HashSet<String>, now: DateTime<Utc>) -> usize {
    let before = items.len();
    let mut unique = seen.clone();
    items.retain(|m| {
        m.published
            .as_deref()
            .and_then(publication)
            .is_some_and(|date| recent(date, now))
            && unique.insert(canonical(&m.url))
    });
    items.sort_by_key(|m| std::cmp::Reverse(m.published.as_deref().and_then(publication)));
    before - items.len()
}

pub fn crossref_date(work: &Value) -> Option<String> {
    for field in ["published", "published-online", "published-print", "issued"] {
        if let Some(parts) = work[field]["date-parts"][0].as_array() {
            let year = i32::try_from(parts.first()?.as_i64()?).ok()?;
            // Year/month-only dates are too imprecise to claim the paper is new this week.
            let month = u32::try_from(parts.get(1)?.as_u64()?).ok()?;
            let day = u32::try_from(parts.get(2)?.as_u64()?).ok()?;
            if let Some(date) = NaiveDate::from_ymd_opt(year, month, day) {
                return Some(date.to_string());
            }
        }
    }
    None
}

pub fn coverage_note(sources: &[Value], at: DateTime<Utc>) -> String {
    let dates: Vec<_> = sources
        .iter()
        .filter_map(|s| s["publishedAt"].as_str().and_then(publication))
        .collect();
    let recent = dates
        .iter()
        .filter(|&&d| d >= at - Duration::hours(24) && d <= at)
        .count();
    let background = dates
        .iter()
        .filter(|&&d| d >= at - Duration::days(7) && d < at - Duration::hours(24))
        .count();
    let other = dates.len() - recent - background;
    let range = match (dates.iter().min(), dates.iter().max()) {
        (Some(first), Some(last)) => format!(
            "来源发表区间{}–{} UTC；",
            first.format("%Y-%m-%d %H:%M"),
            last.format("%Y-%m-%d %H:%M")
        ),
        _ => String::new(),
    };
    format!("截至{} UTC；{range}近24小时{recent}条，1–7天背景{background}条，其他日期{other}条，日期未核实{}条。按原文发表时间统计，发现或抓取时间不算新发布；来源覆盖有限。", at.format("%Y-%m-%d %H:%M"), sources.len() - dates.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn apply_page_date(material: &mut Material, html: &str) {
        let url = material.url.clone();
        super::apply_page_date(material, &url, html);
    }
    fn material(url: &str, date: Option<&str>) -> Material {
        Material {
            title: "A".into(),
            url: url.into(),
            text: "evidence".into(),
            published: date.map(str::to_owned),
            discovered: None,
            discovered_via: None,
            publication_source: "feed_publication",
            publisher: "Lab".into(),
            fulltext: false,
            pdf: None,
        }
    }
    #[test]
    fn digest_does_not_recycle_old_unknown_future_or_previously_delivered_sources() {
        let now = publication("2026-10-12T00:00:00Z").unwrap();
        let seen =
            prior_urls(&[json!({"kind":"digest","sources":[{"url":"https://example.com/seen"}]})]);
        let mut items = vec![
            material(
                "https://example.com/new",
                Some("Sun, 11 Oct 2026 10:00:00 GMT"),
            ),
            material("https://example.com/old", Some("2020-01-01")),
            material("https://example.com/unknown", None),
            material("https://example.com/future", Some("2026-12-01")),
            material(
                "https://example.com/seen?utm_source=rss",
                Some("2026-10-11"),
            ),
            material("https://example.com/new#section", Some("2026-10-11")),
        ];
        assert_eq!(select(&mut items, &seen, now), 5);
        assert_eq!(items.len(), 1);
        assert!(items[0].url.ends_with("/new"));
    }
    #[test]
    fn publication_precision_and_actual_calendar_dates_are_required() {
        assert!(crossref_date(&json!({"published":{"date-parts":[[2026]]}})).is_none());
        assert!(crossref_date(&json!({"published":{"date-parts":[[2026,2,30]]}})).is_none());
        assert_eq!(
            crossref_date(&json!({"published":{"date-parts":[[2026,10,11]]}})),
            Some("2026-10-11".into())
        );
    }
    #[test]
    fn arxiv_first_submission_overrides_later_rss_publication_and_counts_as_background() {
        // Both arxiv.org/abs/2610.12536 and /2610.12542 expose these original metadata values.
        let html = r#"<meta name="citation_date" content="2026/10/08">
            <meta name="citation_online_date" content="2026/10/08">"#;
        let mut paper = material(
            "https://arxiv.org/abs/2610.12536",
            Some("Mon, 12 Oct 2026 00:00:00 -0400"),
        );
        apply_page_date(&mut paper, html);
        assert_eq!(
            paper.published.as_deref(),
            Some("2026-10-08T00:00:00+00:00")
        );
        assert_eq!(paper.publication_source, "original_page_metadata");
        let now = publication("2026-10-12T15:41:30Z").unwrap();
        let mut items = vec![paper];
        assert_eq!(select(&mut items, &HashSet::new(), now), 0);
        let note = coverage_note(&[json!({"publishedAt":items[0].published})], now);
        assert!(note.contains("近24小时0条"), "{note}");
        assert!(note.contains("1–7天背景1条"), "{note}");
    }
    #[test]
    fn arxiv_updates_cannot_redate_an_old_first_submission() {
        let mut items = vec![material(
            "https://arxiv.org/abs/old",
            Some("Mon, 12 Oct 2026 00:00:00 -0400"),
        )];
        apply_page_date(
            &mut items[0],
            r#"<meta name="citation_date" content="2026/10/01">
                <meta name="citation_online_date" content="2026/10/12">
                <meta itemprop="dateModified" content="2026-10-12">
                <script type="application/ld+json">{"@type":"ScholarlyArticle","dateModified":"2026-10-12"}</script>"#,
        );
        assert_eq!(
            items[0].published.as_deref(),
            Some("2026-10-01T00:00:00+00:00")
        );
        assert_eq!(
            select(
                &mut items,
                &HashSet::new(),
                publication("2026-10-12T15:41:30Z").unwrap()
            ),
            1
        );
        assert!(page_publication("https://arxiv.org/abs/old",
            r#"<meta name="citation_online_date" content="2026/10/12"><meta itemprop="dateModified" content="2026-10-12">"#
        ).is_none());
    }
    #[test]
    fn aggregator_submission_time_never_redates_old_original_articles() {
        let now = publication("2026-10-12T00:00:00Z").unwrap();
        let mut youtube = material("https://youtube.com/watch?v=old", None);
        youtube.discovered = Some("2026-10-12T00:00:00Z".into());
        youtube.publication_source = "aggregator_submission";
        apply_page_date(&mut youtube,"<meta itemprop='datePublished' content='2000-04-01'><meta itemprop='dateModified' content='2026-10-12'>");
        assert_eq!(youtube.publication_source, "original_page_metadata");
        let mut april = material("https://example.com/april", Some("2026-10-12"));
        apply_page_date(
            &mut april,
            r#"<script type="application/ld+json">{"@type":"NewsArticle","datePublished":"2026-04-01","dateModified":"2026-10-12"}</script>"#,
        );
        let mut items = vec![
            youtube,
            april,
            material("https://example.com/unknown", None),
        ];
        assert_eq!(select(&mut items, &HashSet::new(), now), 3);
        assert!(items.is_empty());
    }
    #[test]
    fn undated_candidates_require_actual_publication_after_original_page_fetch() {
        let now = publication("2026-10-12T00:00:00Z").unwrap();
        let mut items = vec![material("https://example.com/new", None)];
        prepare(&mut items, &HashSet::new(), now);
        assert_eq!(items.len(), 1);
        apply_page_date(
            &mut items[0],
            "<meta property='article:published_time' content='2026-10-11T10:00:00Z'>",
        );
        assert_eq!(select(&mut items, &HashSet::new(), now), 0);
        assert!(page_publication("https://example.com/article", "<time datetime='2026-10-12'>comment</time><meta property='article:modified_time' content='2026-10-12'>").is_none());
    }
    #[test]
    fn older_space_separated_publication_wins_over_current_duplicate_meta() {
        let html = "<meta property='article:published_time' content='2026-10-12T10:45:50+08:00'><meta property='article:published_time' content='2024-09-13 07:27:41'>";
        assert_eq!(
            page_publication("https://example.com/old", html).as_deref(),
            Some("2024-09-13T07:27:41+00:00")
        );
        let mut items = vec![material("https://example.com/old", None)];
        apply_page_date(&mut items[0], html);
        assert_eq!(
            select(
                &mut items,
                &HashSet::new(),
                publication("2026-10-12T00:00:00Z").unwrap()
            ),
            1
        );
        assert!(publication("2024-02-30 07:27:41").is_none());
    }
    #[test]
    fn coverage_distinguishes_recent_publications_from_background_and_unknown_dates() {
        let sources = vec![
            json!({"publishedAt":"2026-10-12T06:00:00Z"}),
            json!({"publishedAt":"2026-10-09T12:00:00Z"}),
            json!({"publishedAt":null,"discoveredAt":"2026-10-12T11:00:00Z"}),
        ];
        let note = coverage_note(&sources, publication("2026-10-12T12:00:00Z").unwrap());
        for expected in [
            "截至2026-10-12 12:00 UTC",
            "近24小时1条",
            "1–7天背景1条",
            "日期未核实1条",
            "2026-10-09",
            "2026-10-12",
        ] {
            assert!(note.contains(expected), "missing {expected}: {note}");
        }
    }
}
