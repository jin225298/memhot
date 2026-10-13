//! Server-owned public domains. Aliases are exact declarations, never inferred user queries.
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct CatalogTopic {
    pub id: &'static str,
    pub title: &'static str,
    pub aliases: &'static [&'static str],
    pub queries: &'static [&'static str],
    pub feeds: &'static [&'static str],
}
#[derive(Clone, Copy)]
pub(super) struct FeedPolicy {
    pub domain: &'static str,
    pub layer: &'static str,
    pub official_abstract: bool,
}
const OPENAI: &str = "https://openai.com/news/rss.xml";
const GOOGLE: &str = "https://research.google/blog/rss/";
const HF: &str = "https://huggingface.co/blog/feed.xml";
const NVIDIA: &str = "https://blogs.nvidia.com/feed/";
const BAIR: &str = "https://bair.berkeley.edu/blog/feed.xml";
const TC: &str = "https://techcrunch.com/category/artificial-intelligence/feed/";
const NASA: &str = "https://www.nasa.gov/news-release/feed/";
const APS: &str = "https://feeds.aps.org/rss/recent/physics.xml";
// Feed addresses are publisher-owned. AI candidates were checked against AIHOT's public
// example directory. The opt-in licensed MCP discovery path is separate from these feeds.
// Science directories: nasa.gov/rss-feeds/, journals.aps.org/feeds,
// info.arxiv.org/help/rss.html. Feed availability is checked at retrieval, not assumed.
pub(super) static CATALOG: &[CatalogTopic] = &[
    CatalogTopic {
        id: "ai",
        title: "人工智能",
        aliases: &["ai", "人工智能", "ai技术"],
        queries: &["artificial intelligence model product open source release"],
        feeds: &[
            OPENAI,
            HF,
            GOOGLE,
            TC,
            NVIDIA,
            BAIR,
            "https://rss.arxiv.org/rss/cs.AI",
        ],
    },
    CatalogTopic {
        id: "robotics",
        title: "机器人",
        aliases: &["robotics", "机器人", "具身智能"],
        queries: &["robotics embodied intelligence robot release research"],
        feeds: &[NVIDIA, HF, BAIR, TC, "https://rss.arxiv.org/rss/cs.RO"],
    },
    CatalogTopic {
        id: "astronomy",
        title: "天文学",
        aliases: &["astronomy", "天文", "天文学", "宇宙"],
        queries: &["astronomy telescope astrophysics discovery"],
        feeds: &[
            NASA,
            "https://www.eso.org/public/news/feed/",
            "https://rss.arxiv.org/rss/astro-ph",
        ],
    },
    CatalogTopic {
        id: "physics",
        title: "物理学",
        aliases: &["physics", "物理", "物理学"],
        queries: &["physics experiment quantum matter research"],
        feeds: &[
            APS,
            "https://rss.arxiv.org/rss/physics",
            "https://rss.arxiv.org/rss/quant-ph",
        ],
    },
];
pub fn catalog() -> &'static [CatalogTopic] {
    CATALOG
}
pub fn match_topics(interests: &[String]) -> Vec<&'static CatalogTopic> {
    CATALOG
        .iter()
        .filter(|topic| {
            interests.iter().any(|interest| {
                topic
                    .aliases
                    .contains(&interest.trim().to_lowercase().as_str())
            })
        })
        .collect()
}
pub(super) fn policy(url: &str) -> Option<FeedPolicy> {
    let (domain, layer, official_abstract) = match url {
        OPENAI => ("openai.com", "产品发布", true),
        GOOGLE | BAIR => (
            if url == GOOGLE {
                "research.google"
            } else {
                "bair.berkeley.edu"
            },
            "研究",
            true,
        ),
        HF => ("huggingface.co", "开源工具", true),
        NVIDIA => ("nvidia.com", "产品发布", true),
        TC => ("techcrunch.com", "行业报道", false),
        NASA => ("nasa.gov", "机构发布", true),
        APS => ("aps.org", "学会解读", true),
        "https://www.eso.org/public/news/feed/" => ("eso.org", "机构发布", true),
        u if u.starts_with("https://rss.arxiv.org/rss/") => ("arxiv.org", "论文", true),
        _ => return None,
    };
    Some(FeedPolicy {
        domain,
        layer,
        official_abstract,
    })
}
pub(super) fn publisher_matches(url: &str, domain: &str) -> bool {
    crate::network::validate_url(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .is_some_and(|host| host == domain || host.ends_with(&format!(".{domain}")))
}
pub(super) fn relevant(topic: &CatalogTopic, material: &crate::discovery::Material) -> bool {
    let url = material.discovered_via.as_deref().unwrap_or_default();
    if url.contains("rss.arxiv.org/")
        || (topic.id == "ai" && url.starts_with("https://aihot.news/"))
        || (topic.id == "ai" && [OPENAI, GOOGLE, HF, TC, BAIR].contains(&url))
        || (topic.id == "physics" && url == APS)
    {
        return true;
    }
    let terms = match topic.id {
        "ai" => &[
            "artificial intelligence",
            "machine learning",
            "generative",
            "language model",
            "robot",
            "ai ",
            "llm",
            "deepseek",
            "chatgpt",
            "claude",
            "人工智能",
            "大模型",
            "agent",
        ][..],
        "robotics" => &[
            "robot",
            "embodied",
            "manipulation",
            "humanoid",
            "lerobot",
            "机器人",
        ][..],
        "astronomy" => &[
            "astronom",
            "telescope",
            "galax",
            "planet",
            "cosmic",
            "star",
            "black hole",
            "universe",
        ][..],
        _ => &["physics", "quantum", "particle", "matter", "experiment"][..],
    };
    let text = format!("{} {}", material.title, material.text).to_lowercase();
    terms.iter().any(|term| text.contains(term))
}
