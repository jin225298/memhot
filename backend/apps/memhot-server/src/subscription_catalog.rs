//! Explicitly curated public feeds; a matching arbitrary URL is not sharing consent.
use serde::Serialize;
use serde_json::{json, Value};
#[derive(Clone, Copy, Debug, Serialize)]
pub struct CatalogSource {
    pub id: &'static str,
    pub title: &'static str,
    pub url: &'static str,
    pub category: &'static str,
    pub description: &'static str,
}
// Verified feed endpoints recorded in docs/memhot/source-candidates-20261013.json.
const SOURCES: &[CatalogSource] = &[
    CatalogSource {
        id: "federal-reserve",
        title: "美联储新闻稿",
        url: "https://www.federalreserve.gov/feeds/press_all.xml",
        category: "投资",
        description: "官方政策与经济发布；按事件更新。",
    },
    CatalogSource {
        id: "stratechery",
        title: "Stratechery",
        url: "https://stratechery.com/feed/",
        category: "商业",
        description: "科技与商业分析的公开 RSS；付费文章可能只有预览。",
    },
    CatalogSource {
        id: "adexchanger",
        title: "AdExchanger",
        url: "https://www.adexchanger.com/feed",
        category: "商业",
        description: "广告、媒体与商业报道；RSS 摘要不代表已获取全文。",
    },
    CatalogSource {
        id: "nielsen-norman-group",
        title: "Nielsen Norman Group",
        url: "https://www.nngroup.com/feed/rss/",
        category: "设计",
        description: "用户体验与产品设计文章。",
    },
    CatalogSource {
        id: "open-robotics",
        title: "Open Robotics 社区",
        url: "https://discourse.openrobotics.org/latest.rss",
        category: "机器人",
        description: "机器人社区讨论；社区观点不等同于官方公告。",
    },
    CatalogSource {
        id: "nasa-news",
        title: "NASA 新闻发布",
        url: "https://www.nasa.gov/news-release/feed/",
        category: "天文",
        description: "NASA 官方航天与科学新闻。",
    },
    CatalogSource {
        id: "esa-space-science",
        title: "ESA 空间科学",
        url: "https://www.esa.int/rssfeed/Our_Activities/Space_Science",
        category: "天文",
        description: "欧洲航天局空间科学进展。",
    },
];
pub fn entries() -> &'static [CatalogSource] {
    SOURCES
}
pub fn find(id: &str) -> Option<&'static CatalogSource> {
    SOURCES.iter().find(|s| s.id == id)
}
pub fn sources() -> Value {
    json!({"sources": SOURCES})
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn catalog_is_explicit_unique_and_backed_by_verified_public_feeds() {
        let inventory: Value = serde_json::from_str(include_str!(
            "../../../../docs/memhot/source-candidates-20261013.json"
        ))
        .unwrap();
        let mut ids = std::collections::HashSet::new();
        let mut urls = std::collections::HashSet::new();
        for source in entries() {
            assert!(ids.insert(source.id));
            assert!(urls.insert(source.url));
            assert!(crate::subscription_preview::public_url(source.url).is_ok());
            assert!(inventory["sources"]
                .as_array()
                .unwrap()
                .iter()
                .any(|s| s["url"] == source.url
                    && s["verification"] == "feed_checked"
                    && s["kind"] == "rss"));
            assert_eq!(find(source.id).unwrap().url, source.url);
        }
        for category in ["商业", "投资", "设计", "机器人", "天文"] {
            assert!(entries().iter().any(|s| s.category == category));
        }
        assert!(find("36kr").is_none());
    }
}
