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
pub async fn discover(profile: &Value, queries: &[String]) -> (Vec<Material>, Vec<String>) {
    discover_limited(profile, queries, 6).await
}
pub async fn discover_limited(
    profile: &Value,
    queries: &[String],
    max_fetch: usize,
) -> (Vec<Material>, Vec<String>) {
    let digest = profile["_discoveryKind"] == "digest";
    let mut found = Vec::new();
    let mut feed_found = Vec::new();
    let mut warnings = Vec::new();
    if let Some(feeds) = profile["feeds"].as_array() {
        // Rotate a bounded batch so later subscriptions are not permanently ignored.
        let start =
            (chrono::Utc::now().timestamp().div_euclid(86400) as usize * 3) % feeds.len().max(1);
        for feed in feeds
            .iter()
            .cycle()
            .skip(start)
            .take(feeds.len().min(3))
            .filter_map(Value::as_str)
        {
            match tokio::time::timeout(std::time::Duration::from_secs(15), network::fetch(feed))
                .await
            {
                Ok(Ok(data)) => feed_found.extend(crate::feed::parse_from(&data.text, Some(feed))),
                _ => warnings.push("有订阅来源暂时无法读取".into()),
            }
        }
    }
    let tavily = &profile["search"];
    let use_tavily =
        tavily["provider"] == "tavily" && tavily["apiKey"].as_str().is_some_and(|s| !s.is_empty());
    for query in queries.iter().take(if digest { 2 } else { 4 }) {
        if crate::search_backend::configured() {
            match crate::search_backend::search(query, digest).await {
                Ok((materials, notes)) => {
                    found.extend(materials);
                    warnings.extend(notes);
                }
                Err(_) => {
                    warnings.push("免费聚合搜索暂不可用，本次使用公开学术与订阅来源补充".into())
                }
            }
        }
        if use_tavily {
            let mut request = json!({"api_key":tavily["apiKey"],"query":clip(query,300),"max_results":6,"search_depth":"basic","include_answer":false,"include_raw_content":false});
            if digest {
                request["topic"] = json!("news");
                request["time_range"] = json!("week");
            }
            match network::json_post("https://api.tavily.com/search", Default::default(), request)
                .await
            {
                Ok(data) => {
                    if let Some(results) = data["results"].as_array() {
                        for r in results {
                            if let Some(url) = r["url"].as_str() {
                                found.push(Material {
                                    title: r["title"].as_str().unwrap_or("搜索资料").into(),
                                    url: url.into(),
                                    text: r["content"].as_str().unwrap_or_default().into(),
                                    published: r["published_date"].as_str().map(str::to_owned),
                                    discovered: None,
                                    discovered_via: Some("https://api.tavily.com/search".into()),
                                    publication_source: "search_reported_publication",
                                    publisher: network::validate_url(url)
                                        .ok()
                                        .and_then(|u| u.host_str().map(str::to_owned))
                                        .unwrap_or_default(),
                                    fulltext: false,
                                    pdf: None,
                                });
                            }
                        }
                    }
                }
                Err(_) => {
                    warnings.push("Tavily搜索失败，改用公开资料接口".into());
                    found.extend(public_search(query, digest).await);
                }
            }
        } else {
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
    let excluded = crate::feed::filter_topics(&mut feed_found, feed_queries);
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
    found = diverse(found, feed_found, if digest { 10 } else { 16 });
    let mut seen = HashSet::new();
    found.retain(|m| network::validate_url(&m.url).is_ok() && seen.insert(m.url.clone()));
    found.truncate(if digest { 10 } else { 16 });
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
        let excluded = freshness::select(&mut found, &prior, chrono::Utc::now());
        if excluded > 0 {
            warnings.push("已按原文发表日期再次筛选；排除旧文、重复、日期不明内容，HN提交/聚合日期不作为发表日期".into());
        }
    }
    found.retain(|m| {
        !m.text.trim().is_empty()
            && (m.publication_source != "search_reported_publication" || m.fulltext)
    });
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

fn diverse(search: Vec<Material>, mut feeds: Vec<Material>, limit: usize) -> Vec<Material> {
    feeds.sort_by(|a, b| b.published.cmp(&a.published));
    let mut a = search.into_iter();
    let mut b = feeds.into_iter();
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut domains = std::collections::HashMap::new();
    loop {
        let mut any = false;
        for candidate in [a.next(), a.next(), b.next()].into_iter().flatten() {
            any = true;
            let Ok(url) = network::validate_url(&candidate.url) else {
                continue;
            };
            let domain = url.host_str().unwrap_or_default().to_owned();
            let count = domains.entry(domain).or_insert(0usize);
            if *count >= 2 || !seen.insert(candidate.url.clone()) {
                continue;
            }
            *count += 1;
            out.push(candidate);
            if out.len() >= limit {
                return out;
            }
        }
        if !any {
            break;
        }
    }
    out
}
