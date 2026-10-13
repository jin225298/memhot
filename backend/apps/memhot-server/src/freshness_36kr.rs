//! 36kr renders a request-time article:published_time; only the matching article state is usable.
use chrono::{DateTime, Utc};
use serde_json::Value;

pub(super) fn matches(url: &str) -> bool {
    reqwest::Url::parse(url).is_ok_and(|url| {
        url.host_str()
            .is_some_and(|host| host == "36kr.com" || host.ends_with(".36kr.com"))
    })
}

pub(super) fn publication(url: &str, html: &str) -> Option<String> {
    let url = reqwest::Url::parse(url).ok()?;
    let id = url.path().strip_prefix("/p/")?.trim_end_matches('/');
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let doc = dom_query::Document::from(html);
    for script in doc.select("script:not([src])").iter().take(40) {
        let text = script.text();
        if text.len() > 512 * 1024 {
            continue;
        }
        // Parse JSON data only. Never execute hydration scripts or scan recommended articles.
        // The live script may set other boot variables before the article state assignment.
        let value = text
            .match_indices("window.initialState")
            .take(8)
            .find_map(|(index, _)| {
                let state = text[index + "window.initialState".len()..]
                    .trim_start()
                    .strip_prefix('=')?;
                serde_json::from_str::<Value>(state.trim().trim_end_matches(';').trim()).ok()
            });
        let Some(value) = value else {
            continue;
        };
        let article = &value["articleDetail"]["articleDetailData"]["data"];
        let article_id = article["itemId"]
            .as_str()
            .map(str::to_owned)
            .or_else(|| article["itemId"].as_u64().map(|id| id.to_string()));
        if article_id.as_deref() != Some(id) {
            continue;
        }
        let millis = article["publishTime"]
            .as_i64()
            .or_else(|| article["publishTime"].as_str()?.parse().ok())?;
        return DateTime::<Utc>::from_timestamp_millis(millis).map(|date| date.to_rfc3339());
    }
    None
}

#[cfg(test)]
mod tests {
    use crate::{discovery::Material, freshness};
    use serde_json::json;
    use std::collections::HashSet;

    const URL: &str = "https://www.36kr.com/p/3231130347355525";
    fn page(article_id: &str, published: i64) -> String {
        format!(
            r#"<meta property="article:published_time" content="2026-10-13T11:36:02+08:00">
            <script>window.someBootFlag = true; window.initialState = {};</script>"#,
            json!({"articleDetail":{
                "articleDetailData":{"data":{"itemId":article_id,"publishTime":published}},
                "articleRecommendData":{"newestItemList":[
                    {"itemId":"4022394995069062","publishTime":1791792052026_i64},
                    {"itemId":"123456","publishTime":1577836800000_i64},
                    {"itemId":"3231130347355525","publishTime":1791792052026_i64}
                ]}
            }})
        )
    }
    fn material() -> Material {
        Material {
            title: "Article".into(),
            url: URL.into(),
            text: "Original evidence".into(),
            published: Some("2026-10-13T03:36:02Z".into()),
            discovered: Some("2026-10-13T03:36:02Z".into()),
            discovered_via: None,
            publication_source: "original_page_metadata",
            publisher: "36kr".into(),
            fulltext: true,
            pdf: None,
        }
    }
    #[test]
    fn matching_article_state_overrides_dynamic_meta_and_never_uses_recommendation_dates() {
        let html = page("3231130347355525", 1743495343753);
        assert_eq!(
            freshness::page_publication(URL, &html).as_deref(),
            Some("2025-04-01T08:15:43.753+00:00")
        );
        let numeric_id = html.replace(
            "\"itemId\":\"3231130347355525\"",
            "\"itemId\":3231130347355525",
        );
        assert_eq!(
            freshness::page_publication(URL, &numeric_id),
            freshness::page_publication(URL, &html)
        );
        let mut article = material();
        freshness::apply_page_date(&mut article, URL, &html);
        assert_eq!(article.publication_source, "original_page_metadata");
        let mut items = vec![article];
        assert_eq!(
            freshness::select(
                &mut items,
                &HashSet::new(),
                freshness::publication("2026-10-13T04:00:00Z").unwrap()
            ),
            1
        );
        assert!(items.is_empty());

        let published = freshness::publication("2026-10-12T10:00:00Z")
            .unwrap()
            .timestamp_millis();
        assert_eq!(
            freshness::page_publication(URL, &page("3231130347355525", published)).as_deref(),
            Some("2026-10-12T10:00:00+00:00")
        );
    }
    #[test]
    fn missing_malformed_or_mismatched_current_article_clears_stale_dates() {
        for html in [
            page("999999", 1743495343753),
            "<meta property='article:published_time' content='2026-10-13T11:36:02+08:00'>".into(),
            "<script>window.initialState = {articleDetail: 'not JSON'};</script>".into(),
        ] {
            assert!(freshness::page_publication(URL, &html).is_none());
            let mut article = material();
            // Apply the fetched URL even if the source was initially discovered through a redirect.
            article.url = "https://example.com/redirect".into();
            freshness::apply_page_date(&mut article, URL, &html);
            assert!(article.published.is_none());
            assert_eq!(article.publication_source, "unknown");
        }
    }
    #[test]
    fn recently_published_metadata_on_other_sites_remains_valid() {
        let html = "<meta property='article:published_time' content='2026-10-13T03:36:02Z'>";
        for url in [
            "https://example.com/article",
            "https://www.36kr.com.example.org/p/3231130347355525",
        ] {
            assert_eq!(
                freshness::page_publication(url, html).as_deref(),
                Some("2026-10-13T03:36:02+00:00")
            );
        }
        assert!(freshness::page_publication(
            "https://www.36kr.com/p/999999",
            &page("3231130347355525", 1743495343753)
        )
        .is_none());
    }
}
