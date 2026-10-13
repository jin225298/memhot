//! Bounded fixed-catalog retrieval. Search is discovery, never evidence by itself.
use super::{cached_page, catalog_data, register_public, CatalogTopic};
use crate::{
    clip,
    discovery::{self, Material},
    error::{Error, Result},
    feed, freshness, App,
};
use chrono::Utc;
use std::collections::{HashMap, HashSet, VecDeque};
use std::time::Duration;

pub(super) struct Retrieved {
    pub materials: Vec<Material>,
    pub metadata: HashMap<String, (i64, String)>,
    pub gaps: Vec<String>,
    pub aihot_context: Option<serde_json::Value>,
}
struct Candidate {
    material: Material,
    abstract_allowed: bool,
}
type FeedResult = (
    usize,
    Vec<Candidate>,
    HashMap<String, (i64, String)>,
    Vec<String>,
);

fn feed_candidate(mut m: Material, url: &str) -> Candidate {
    let policy = catalog_data::policy(url);
    let same_publisher = policy.is_some_and(|p| catalog_data::publisher_matches(&m.url, p.domain));
    let abstract_allowed = same_publisher && policy.is_some_and(|p| p.official_abstract);
    // Tracking-only links are canonicalized. Other query strings remain disallowed by cache.
    m.url = freshness::canonical(&m.url);
    if same_publisher && m.url.starts_with("http://") {
        m.url = m.url.replacen("http://", "https://", 1);
    }
    if !abstract_allowed {
        m.text.clear();
    }
    if !same_publisher {
        m.published = None;
        m.publication_source = "unknown";
    }
    Candidate {
        material: m,
        abstract_allowed,
    }
}
fn search_candidate(mut m: Material) -> Candidate {
    // Neither a search snippet nor a search engine's date is an original fact source.
    m.text.clear();
    m.published = None;
    m.fulltext = false;
    m.publication_source = "unknown";
    m.url = freshness::canonical(&m.url);
    Candidate {
        material: m,
        abstract_allowed: false,
    }
}
async fn fetch_feed(
    app: App,
    topic: &'static CatalogTopic,
    index: usize,
    url: &'static str,
) -> FeedResult {
    let mut metadata = HashMap::new();
    let mut gaps = Vec::new();
    let mut found = Vec::new();
    match cached_page(&app, url).await {
        Ok(page) => {
            if page.cache_status == "stale" {
                gaps.push("部分公开订阅使用24小时内的旧缓存，刷新暂不可用".into());
            }
            let mut items = feed::parse_from(&page.text, Some(url));
            items.retain(|m| catalog_data::relevant(topic, m));
            freshness::prepare(&mut items, &HashSet::new(), Utc::now());
            items.sort_by_key(|m| {
                std::cmp::Reverse(m.published.as_deref().and_then(freshness::publication))
            });
            for m in items.into_iter().take(10) {
                let candidate = feed_candidate(m, url);
                metadata.insert(
                    candidate.material.url.clone(),
                    (page.fetched_at, page.cache_status.clone()),
                );
                found.push(candidate);
            }
            if found.is_empty() {
                gaps.push("一个目录订阅当前没有可用的新材料".into());
            }
        }
        Err(_) => gaps.push("一个目录订阅暂时无法获取".into()),
    }
    (index, found, metadata, gaps)
}
async fn feeds(app: &App, topic: &'static CatalogTopic) -> Vec<FeedResult> {
    let mut pending = topic.feeds.iter().enumerate();
    let mut tasks = tokio::task::JoinSet::new();
    let mut out = Vec::new();
    loop {
        while tasks.len() < 4 {
            let Some((index, &url)) = pending.next() else {
                break;
            };
            if register_public(app, url).is_ok() {
                tasks.spawn(fetch_feed(app.clone(), topic, index, url));
            }
        }
        let Some(result) = tasks.join_next().await else {
            break;
        };
        if let Ok(result) = result {
            out.push(result);
        }
    }
    out.sort_by_key(|r| r.0);
    out
}
async fn search(topic: &CatalogTopic) -> Vec<Candidate> {
    let mut found = Vec::new();
    for query in topic.queries {
        let result = tokio::time::timeout(Duration::from_secs(20), async {
            crate::agent_web::search(query).await.unwrap_or_default()
        })
        .await
        .unwrap_or_default();
        found.extend(result.into_iter().take(6).map(search_candidate));
    }
    found
}
fn balanced(groups: Vec<Vec<Candidate>>, limit: usize) -> Vec<Candidate> {
    let mut groups: Vec<VecDeque<_>> = groups.into_iter().map(VecDeque::from).collect();
    let mut seen = HashSet::new();
    let mut publishers = HashMap::new();
    let mut out = Vec::new();
    loop {
        let mut any = false;
        for group in &mut groups {
            let Some(candidate) = group.pop_front() else {
                continue;
            };
            any = true;
            let Ok(url) = crate::network::validate_url(&candidate.material.url) else {
                continue;
            };
            let count = publishers
                .entry(url.host_str().unwrap_or_default().to_owned())
                .or_insert(0);
            if *count >= 2 || !seen.insert(candidate.material.url.clone()) {
                continue;
            }
            *count += 1;
            out.push(candidate);
            if out.len() >= limit {
                return out;
            }
        }
        if !any {
            return out;
        }
    }
}
async fn enrich_inner(
    app: App,
    candidate: Candidate,
) -> (Material, Option<(i64, String)>, Option<String>) {
    let mut material = candidate.material;
    let Ok(url) = register_public(&app, &material.url) else {
        material.text.clear();
        material.published = None;
        return (material, None, Some("不符合共享缓存规则的URL已排除".into()));
    };
    match cached_page(&app, &url).await {
        Ok(page) => {
            freshness::apply_page_date(&mut material, &page.url, &page.text);
            let text = if page.content_type.contains("html") {
                discovery::plain(&page.text)
            } else {
                page.text
            };
            if text.chars().count() >= 100 {
                material.text = clip(&text, 18000);
                material.fulltext = true;
                material.url = page.url;
                material.publisher = crate::network::validate_url(&material.url)
                    .ok()
                    .and_then(|u| u.host_str().map(str::to_owned))
                    .unwrap_or_default();
            }
            let warning = (page.cache_status == "stale")
                .then(|| "部分原网页刷新失败，使用缓存引文并保留原发布日期".into());
            (
                material,
                Some((page.fetched_at, page.cache_status)),
                warning,
            )
        }
        Err(_) => {
            let warning = if candidate.abstract_allowed {
                "部分官方来源只取得其订阅摘要，未读取完整正文"
            } else {
                "部分发现链接未取得正文，已排除其搜索或媒体摘要"
            };
            (material, None, Some(warning.into()))
        }
    }
}
async fn enrich(
    app: App,
    candidate: Candidate,
) -> (Material, Option<(i64, String)>, Option<String>) {
    let fallback = candidate.material.clone();
    tokio::time::timeout(Duration::from_secs(18), enrich_inner(app, candidate))
        .await
        .unwrap_or((
            fallback,
            None,
            Some("部分原网页读取超时，已尝试其他来源".into()),
        ))
}

/// Only called with a server catalog scope. Search snippets are discoveries, not evidence.
pub(super) async fn agent_search(
    app: &App,
    topic: &'static CatalogTopic,
    query: &str,
) -> Result<Vec<Material>> {
    if query.trim().is_empty() || query.chars().count() > 300 {
        return Err(Error::bad("搜索词长度无效"));
    }
    let scoped = format!("{} {}", topic.queries[0], query.trim());
    let mut candidates = tokio::time::timeout(Duration::from_secs(20), async {
        crate::agent_web::search(&scoped).await.unwrap_or_default()
    })
    .await
    .unwrap_or_default();
    candidates.retain(|m| catalog_data::relevant(topic, m));
    let candidates = balanced(
        vec![candidates.into_iter().map(search_candidate).collect()],
        8,
    );
    let mut tasks = tokio::task::JoinSet::new();
    for candidate in candidates.into_iter().take(4) {
        tasks.spawn(enrich(app.clone(), candidate));
    }
    let mut materials = Vec::new();
    while let Some(result) = tasks.join_next().await {
        if let Ok((material, _, _)) = result {
            materials.push(material);
        }
    }
    freshness::select(&mut materials, &HashSet::new(), Utc::now());
    materials.retain(|m| m.text.chars().count() >= 80 && catalog_data::relevant(topic, m));
    materials.sort_by(|a, b| a.url.cmp(&b.url));
    Ok(materials)
}
pub(super) async fn collect(app: &App, topic: &'static CatalogTopic) -> Result<Retrieved> {
    let (aihot, feeds, search) =
        tokio::join!(aihot_seed(app, topic), feeds(app, topic), search(topic));
    let mut metadata = HashMap::new();
    let mut gaps = Vec::new();
    let mut groups = Vec::new();
    let mut aihot_context = None;
    match aihot {
        Ok((items, context)) => {
            aihot_context = context;
            if !items.is_empty() {
                groups.push(items.into_iter().map(search_candidate).collect());
            }
        }
        Err(_) => gaps.push("AIHOT 发现渠道暂不可用，本次继续使用其他公开来源".into()),
    }
    for (_, items, meta, notes) in feeds {
        groups.push(items);
        metadata.extend(meta);
        gaps.extend(notes);
    }
    groups.push(search);
    // Consider a larger bounded reserve; publisher quotas apply to successful materials,
    // so a failed first link no longer consumes that publisher's only useful seat.
    let mut pending = candidate_reserve(groups, 48).into_iter();
    let mut tasks = tokio::task::JoinSet::new();
    let mut materials = Vec::new();
    let mut publishers = HashMap::<String, usize>::new();
    let mut excluded = 0;
    loop {
        while tasks.len() < 4 && materials.len() + tasks.len() < 24 {
            let Some(candidate) = pending.next() else {
                break;
            };
            tasks.spawn(enrich(app.clone(), candidate));
        }
        let Some(result) = tasks.join_next().await else {
            break;
        };
        if let Ok((material, meta, note)) = result {
            if let Some(meta) = meta {
                metadata.insert(material.url.clone(), meta);
            }
            if let Some(note) = note {
                gaps.push(note);
            }
            let mut checked = vec![material];
            excluded += freshness::select(&mut checked, &HashSet::new(), Utc::now());
            if let Some(material) = checked
                .pop()
                .filter(|m| m.text.trim().chars().count() >= 80 && catalog_data::relevant(topic, m))
            {
                let count = publishers.entry(material.publisher.clone()).or_default();
                if *count < 4 {
                    *count += 1;
                    materials.push(material);
                }
            }
        }
    }
    // Stable source order makes the stored editorial fingerprint independent of fetch completion.
    materials.sort_by(|a, b| a.url.cmp(&b.url));
    if excluded > 0 {
        gaps.push(format!("排除了{excluded}条发布日期未知、过旧或未来的材料"));
    }
    gaps.sort();
    gaps.dedup();
    if materials.is_empty() {
        return Err(Error::upstream(
            "no_fresh_sources",
            "未取得发布日期可核对的新材料",
        ));
    }
    Ok(Retrieved {
        materials,
        metadata,
        gaps,
        aihot_context,
    })
}

async fn aihot_seed(
    app: &App,
    topic: &CatalogTopic,
) -> Result<(Vec<Material>, Option<serde_json::Value>)> {
    if !crate::aihot::enabled() || !matches!(topic.id, "ai" | "robotics") {
        return Ok((Vec::new(), None));
    }
    // Executed before model selection: AIHOT is an actual initial discovery input.
    // The cache wrapper derives every public argument from this server-owned catalog.
    let action = if topic.id == "robotics" {
        "search"
    } else {
        "daily"
    };
    let value = super::aihot_cached(app, topic.id, action).await?;
    let found = crate::aihot::decode(&value)?;
    let mut materials = found.materials;
    if topic.id == "ai" && materials.len() < 4 {
        if let Ok(value) = super::aihot_cached(app, topic.id, "latest").await {
            if let Ok(latest) = crate::aihot::decode(&value) {
                materials.extend(latest.materials);
            }
        }
    }
    materials.retain(|m| catalog_data::relevant(topic, m));
    Ok((materials, found.overview))
}

fn candidate_reserve(groups: Vec<Vec<Candidate>>, limit: usize) -> Vec<Candidate> {
    let mut groups: Vec<VecDeque<_>> = groups.into_iter().map(VecDeque::from).collect();
    let mut seen = HashMap::<String, usize>::new();
    let mut out: Vec<Candidate> = Vec::new();
    while groups.iter().any(|g| !g.is_empty()) {
        for group in &mut groups {
            if let Some(candidate) = group.pop_front() {
                if crate::network::validate_url(&candidate.material.url).is_err() {
                    continue;
                }
                if let Some(&index) = seen.get(&candidate.material.url) {
                    merge_candidate(&mut out[index], candidate);
                } else if out.len() < limit {
                    seen.insert(candidate.material.url.clone(), out.len());
                    out.push(candidate);
                }
            }
        }
    }
    out
}
fn merge_candidate(existing: &mut Candidate, candidate: Candidate) {
    let attribution = [&existing.material, &candidate.material]
        .into_iter()
        .filter_map(|m| m.discovered_via.as_deref())
        .find(|url| url.starts_with("https://aihot.news/"))
        .map(str::to_owned);
    let strength = |c: &Candidate| {
        (
            c.material.published.is_some(),
            c.abstract_allowed,
            c.material.fulltext,
        )
    };
    if strength(&candidate) > strength(existing) {
        *existing = candidate;
    }
    if let Some(attribution) = attribution {
        existing.material.discovered_via = Some(attribution);
    }
}

#[cfg(test)]
#[path = "shared_public_retrieval_tests.rs"]
mod tests;
