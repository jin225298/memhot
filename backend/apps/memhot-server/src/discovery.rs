use crate::{
    clip,
    error::{Error, Result},
    freshness, hash, network, now,
};
use serde_json::{json, Value};
use std::collections::HashSet;
/// Bounded PDF extraction is a reading boundary, never part of quotable text.
#[derive(Clone, Debug, PartialEq)]
pub struct PdfExtraction {
    pub pages_read: u32,
    pub total_pages: u32,
    pub truncated: bool,
}
impl PdfExtraction {
    pub fn from_value(v: &Value) -> Option<Self> {
        let pages_read = u32::try_from(v["pagesRead"].as_u64()?).ok()?;
        let total_pages = u32::try_from(v["totalPages"].as_u64()?).ok()?;
        let truncated = v["truncated"].as_bool()?;
        (v["format"] == "pdf"
            && (1..=40).contains(&pages_read)
            && total_pages >= pages_read
            && (pages_read == total_pages || truncated))
            .then_some(Self {
                pages_read,
                total_pages,
                truncated,
            })
    }
}
#[derive(Clone, Debug)]
pub struct Material {
    pub title: String,
    pub url: String,
    pub text: String,
    pub published: Option<String>,
    pub discovered: Option<String>,
    pub discovered_via: Option<String>,
    pub publication_source: &'static str,
    pub publisher: String,
    pub fulltext: bool,
    pub pdf: Option<PdfExtraction>,
}
impl Material {
    pub fn extraction(&self) -> Value {
        self.pdf
            .as_ref()
            .map(|p| {
                json!({"format":"pdf","pagesRead":p.pages_read,
            "totalPages":p.total_pages,"truncated":p.truncated})
            })
            .unwrap_or(Value::Null)
    }
    pub fn is_search_snippet(&self) -> bool {
        self.publication_source == "search_reported_publication" && !self.fulltext
    }
    pub fn evidence(&self) -> Value {
        // Fetching a paper's landing HTML does not mean its full paper was read.
        let abstract_page = network::validate_url(&self.url).is_ok_and(|url| {
            let host = url.host_str().unwrap_or_default();
            ((host == "arxiv.org" || host == "export.arxiv.org") && url.path().starts_with("/abs/"))
                || ((host == "www.isca-archive.org" || host == "isca-archive.org")
                    && url.path().ends_with(".html"))
        });
        let attribution = self
            .discovered_via
            .as_deref()
            .filter(|u| u.starts_with("https://aihot.news/"))
            .map(|url| json!({"name":"AIHOT","url":url}));
        json!({"id":hash(self.url.as_bytes())[..24].to_owned(),"title":self.title,"url":self.url,"publisher":self.publisher,"publishedAt":self.published,"discoveredAt":self.discovered,"discoveredVia":self.discovered_via,"attribution":attribution,"publicationDateSource":self.publication_source,"retrievedAt":now(),"excerpt":clip(&self.text,1600),"extraction":self.extraction(),"stance":"context","retrievalKind":if self.is_search_snippet(){"search_snippet"}else if self.fulltext && !abstract_page{"page"}else{"metadata_or_abstract"}})
    }
}
pub fn plain(html: &str) -> String {
    let doc = dom_query::Document::from(html);
    doc.select("script,style,noscript,nav,header,footer,form,svg")
        .remove();
    let main = doc.select("article, main, [role=main]");
    let text = if main.exists() {
        main.text().to_string()
    } else {
        doc.select("body").text().to_string()
    };
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}
fn endpoint(base: &str, query: &[(&str, &str)]) -> Result<String> {
    let mut url = network::validate_url(base)?;
    url.query_pairs_mut().extend_pairs(query);
    Ok(url.to_string())
}
async fn json_get(url: &str) -> Result<Value> {
    let fetched = tokio::time::timeout(std::time::Duration::from_secs(15), network::fetch(url))
        .await
        .map_err(|_| Error::upstream("timeout", "搜索来源响应超时"))??;
    serde_json::from_str(&fetched.text)
        .map_err(|_| Error::upstream("invalid_response", "搜索服务返回无效数据"))
}
pub async fn public_search(query: &str, recent: bool) -> Vec<Material> {
    let mut result = Vec::new();
    let short = clip(query, 160);
    let since = chrono::Utc::now() - chrono::Duration::days(7);
    let numeric_filter = format!("created_at_i>{}", since.timestamp());
    let mut hn_params = vec![
        ("query", short.as_str()),
        ("tags", "story"),
        ("hitsPerPage", "5"),
    ];
    if recent {
        hn_params.push(("numericFilters", &numeric_filter));
    }
    if let Ok(url) = endpoint(
        if recent {
            "https://hn.algolia.com/api/v1/search_by_date"
        } else {
            "https://hn.algolia.com/api/v1/search"
        },
        &hn_params,
    ) {
        if let Ok(data) = json_get(&url).await {
            if let Some(hits) = data["hits"].as_array() {
                for h in hits {
                    let Some(url) = h["url"].as_str() else {
                        continue;
                    };
                    result.push(Material {
                        title: h["title"].as_str().unwrap_or("Hacker News").to_owned(),
                        url: url.to_owned(),
                        text: h["story_text"].as_str().map(plain).unwrap_or_default(),
                        published: None,
                        discovered: h["created_at"].as_str().map(str::to_owned),
                        discovered_via: h["objectID"]
                            .as_str()
                            .map(|key| format!("https://news.ycombinator.com/item?id={key}")),
                        publication_source: "aggregator_submission",
                        publisher: "Hacker News（发现渠道）".into(),
                        fulltext: false,
                        pdf: None,
                    });
                }
            }
        }
    }
    let date_filter = format!("from-pub-date:{}", since.format("%Y-%m-%d"));
    let mut crossref_params = vec![("query", short.as_str()), ("rows", "4")];
    if recent {
        crossref_params.extend([
            ("filter", date_filter.as_str()),
            ("sort", "published"),
            ("order", "desc"),
        ]);
    }
    if let Ok(url) = endpoint("https://api.crossref.org/works", &crossref_params) {
        if let Ok(data) = json_get(&url).await {
            if let Some(items) = data["message"]["items"].as_array() {
                for h in items {
                    let Some(url) = h["URL"].as_str() else {
                        continue;
                    };
                    let abstract_text = h["abstract"].as_str().map(plain).unwrap_or_default();
                    result.push(Material {
                        title: h["title"][0].as_str().unwrap_or("Crossref文献").into(),
                        url: url.into(),
                        text: abstract_text,
                        published: freshness::crossref_date(h),
                        discovered: None,
                        discovered_via: Some("https://api.crossref.org/works".into()),
                        publication_source: "crossref_publication",
                        publisher: h["publisher"].as_str().unwrap_or("Crossref").into(),
                        fulltext: false,
                        pdf: None,
                    });
                }
            }
        }
    }
    if let Ok(url) = endpoint(
        "https://export.arxiv.org/api/query",
        &[
            ("search_query", &format!("all:{short}")),
            ("max_results", "4"),
            ("sortBy", if recent { "submittedDate" } else { "relevance" }),
            ("sortOrder", "descending"),
        ],
    ) {
        if let Ok(Ok(fetched)) =
            tokio::time::timeout(std::time::Duration::from_secs(15), network::fetch(&url)).await
        {
            result.extend(parse_feed(&fetched.text));
        }
    }
    result
}
pub use crate::feed::parse as parse_feed;
pub async fn discover(
    profile: &Value,
    queries: &[String],
    subscriptions: Vec<Material>,
) -> (Vec<Material>, Vec<String>) {
    discover_limited(profile, queries, 6, subscriptions).await
}
pub async fn discover_limited(
    profile: &Value,
    queries: &[String],
    max_fetch: usize,
    mut feed_found: Vec<Material>,
) -> (Vec<Material>, Vec<String>) {
    let digest = profile["_discoveryKind"] == "digest";
    let mut found = Vec::new();
    let mut warnings = Vec::new();
    for query in queries.iter().take(if digest { 2 } else { 4 }) {
        let result =
            crate::search_providers::configured_search(&profile["search"], query, digest).await;
        let fallback = result.needs_public_fallback;
        found.extend(result.materials);
        warnings.extend(result.warnings);
        if fallback {
            found.extend(public_search(query, digest).await);
        }
    }
    let interest_queries: Vec<String> = profile["interests"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect();
    let feed_queries = if digest {
        interest_queries.as_slice()
    } else {
        queries
    };
    // Explicit subscriptions are a reader's selection. Their daily edition must
    // not drop English feeds merely because the user's interest labels are Chinese.
    let excluded = if digest {
        0
    } else {
        crate::feed::filter_topics(&mut feed_found, feed_queries)
    };
    if excluded > 0 {
        warnings.push("已排除与当前主题缺少明确关联的订阅条目".into());
    }
    let prior = profile["_seenDigestUrls"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect();
    if digest {
        freshness::prepare(&mut found, &prior, chrono::Utc::now());
        freshness::prepare(&mut feed_found, &prior, chrono::Utc::now());
    }
    // Search candidates always precede subscribed feeds; round-robin keeps a feed from monopolizing seats.
    found = diverse(found, feed_found);
    let mut seen = HashSet::new();
    found.retain(|m| network::validate_url(&m.url).is_ok() && seen.insert(m.url.clone()));
    for material in found.iter_mut().take(max_fetch.min(6)) {
        match tokio::time::timeout(
            std::time::Duration::from_secs(15),
            network::fetch(&material.url),
        )
        .await
        {
            Ok(Ok(page)) => {
                if page.content_type.contains("html") {
                    freshness::apply_page_date(material, &page.url, &page.text);
                }
                let text = if page.content_type.contains("html") {
                    plain(&page.text)
                } else {
                    page.text
                };
                if text.chars().count() >= 100 {
                    material.text = clip(&text, 18000);
                    material.fulltext = true;
                    material.url = page.url;
                    material.publisher = network::validate_url(&material.url)
                        .ok()
                        .and_then(|u| u.host_str().map(str::to_owned))
                        .unwrap_or_default();
                }
            }
            _ => warnings.push(format!(
                "「{}」未能读取全文，仅保留可得摘要",
                clip(&material.title, 60)
            )),
        }
    }
    if digest {
        // Unknown discovery dates remain selectable for original-page verification.
        // Only pages already read are subjected to the post-read date gate here.
        let before = found.len();
        found.retain(|m| {
            !m.fulltext || {
                let mut item = vec![m.clone()];
                freshness::select(&mut item, &prior, chrono::Utc::now());
                !item.is_empty()
            }
        });
        if before != found.len() {
            warnings
                .push("已排除读后确认过旧、重复或日期未知的原文；未读取线索仍可继续核查".into());
        }
    }
    if found.is_empty() {
        warnings.push("未找到可核对的公开正文或摘要；需要更具体的研究问题或自定义RSS信源".into());
    }
    (found, warnings)
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::feed::relevant as relevant_feed;
    #[test]
    fn fetched_publication_landing_pages_are_still_abstracts() {
        let mut material = parse_feed("<rss><channel><item><title>Paper</title><link>https://arxiv.org/abs/2501.11378v1</link><description>Abstract evidence</description></item></channel></rss>").remove(0);
        material.fulltext = true;
        assert_eq!(material.evidence()["retrievalKind"], "metadata_or_abstract");
        material.url =
            "https://www.isca-archive.org/interspeech_2025/wang25b_interspeech.html".into();
        assert_eq!(material.evidence()["retrievalKind"], "metadata_or_abstract");
        material.url = "https://arxiv.org/html/2501.11378v1".into();
        assert_eq!(material.evidence()["retrievalKind"], "page");
    }
    #[test]
    fn feed_and_html_keep_real_text_without_scripts() {
        let xml="<rss><channel><item><title>A &amp; B</title><link>https://example.com/a</link><description><![CDATA[<p>real evidence</p>]]></description></item></channel></rss>";
        let feed = parse_feed(xml);
        assert_eq!(feed.len(), 1);
        assert_eq!(feed[0].title, "A & B");
        assert!(feed[0].text.contains("real evidence"));
        let text=plain("<html><body><script>secret</script><nav>menu</nav><article>Real <b>facts</b></article></body></html>");
        assert!(text.contains("Real facts"));
        assert!(!text.contains("secret"));
    }
    #[test]
    fn meaningful_topic_filter_rejects_stopword_only_and_unrelated_hn_feed_items() {
        let item = |title: &str| {
            parse_feed(&format!("<rss><channel><item><title>{title}</title><link>https://example.com/article</link><description>What the author will do to make it work.</description></item></channel></rss>")).remove(0)
        };
        assert!(!relevant_feed(
            &item("Why the April video will return"),
            &["Will warehouse robotics be viable in the future?".into()]
        ));
        assert!(relevant_feed(
            &item("Robots automate warehouse picking"),
            &["Will warehouse robotics be viable in the future?".into()]
        ));
        assert!(!relevant_feed(
            &item("Theatre motor return"),
            &["to the will".into()]
        ));
        assert!(relevant_feed(
            &item("机器人仓储分拣试验"),
            &["我想知道仓储机器人未来有没有人买单".into()]
        ));
        assert!(!relevant_feed(
            &item("未来行业市场发展最新分析"),
            &["我想知道仓储机器人未来有没有人买单".into()]
        ));
    }
}

fn diverse(search: Vec<Material>, mut feeds: Vec<Material>) -> Vec<Material> {
    feeds.sort_by(|a, b| b.published.cmp(&a.published));
    let mut a = search.into_iter();
    let mut b = feeds.into_iter();
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    loop {
        let mut any = false;
        for candidate in [a.next(), a.next(), b.next()].into_iter().flatten() {
            any = true;
            let Ok(_) = network::validate_url(&candidate.url) else {
                continue;
            };
            if !seen.insert(crate::freshness::canonical(&candidate.url)) {
                continue;
            }
            out.push(candidate);
        }
        if !any {
            break;
        }
    }
    out
}
