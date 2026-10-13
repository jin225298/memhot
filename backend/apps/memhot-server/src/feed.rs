use crate::{
    discovery::{plain, Material},
    network,
};
use quick_xml::{
    events::{BytesStart, Event},
    Reader,
};
use std::collections::HashSet;
#[derive(Default)]
struct Item {
    title: String,
    title_part: String,
    seen_titles: HashSet<String>,
    url: String,
    fallback_id: String,
    text: String,
    date: String,
    comments: String,
}
fn link(e: &BytesStart<'_>, item: &mut Item) {
    let mut href = None;
    let mut rel = None;
    for attr in e.attributes().flatten() {
        if attr.key.as_ref() == b"href" {
            href = Some(String::from_utf8_lossy(&attr.value).to_string());
        }
        if attr.key.as_ref() == b"rel" {
            rel = Some(String::from_utf8_lossy(&attr.value).to_string());
        }
    }
    if rel.is_none_or(|r| r == "alternate") {
        if let Some(url) = href.filter(|u| network::validate_url(u).is_ok()) {
            item.url = url;
        }
    }
}
fn append(item: &mut Item, stack: &[String], text: &str) {
    let field = stack.iter().rev().find(|s| {
        matches!(
            s.as_str(),
            "title"
                | "link"
                | "id"
                | "summary"
                | "description"
                | "encoded"
                | "content"
                | "published"
                | "pubDate"
                | "comments"
        )
    });
    match field.map(String::as_str) {
        Some("title") => item.title_part.push_str(text),
        Some("link") => item.url.push_str(text),
        Some("comments") => item.comments.push_str(text),
        Some("id") => item.fallback_id.push_str(text),
        Some("summary" | "description" | "encoded" | "content") => item.text.push_str(text),
        Some("published" | "pubDate") => item.date.push_str(text),
        _ => {}
    }
}
fn aggregator(identity: &str) -> bool {
    let text = identity.to_lowercase();
    text.contains("hnrss") || text.contains("news.ycombinator.com") || text.contains("hacker news")
}
pub fn parse(xml: &str) -> Vec<Material> {
    parse_from(xml, None)
}
pub fn parse_from(xml: &str, feed_url: Option<&str>) -> Vec<Material> {
    let mut reader = Reader::from_str(xml);
    let mut stack = Vec::new();
    let mut item = None;
    let mut out = Vec::new();
    let mut channel = feed_url.unwrap_or_default().to_owned();
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let name = String::from_utf8_lossy(e.local_name().as_ref()).to_string();
                if name == "entry" || name == "item" {
                    item = Some(Item::default());
                }
                if name == "link" {
                    if let Some(item) = &mut item {
                        link(&e, item);
                    }
                }
                stack.push(name);
                if stack.len() > 64 {
                    return out;
                }
            }
            Ok(Event::Empty(e)) => {
                if e.local_name().as_ref() == b"link" {
                    if let Some(item) = &mut item {
                        link(&e, item);
                    }
                }
            }
            Ok(Event::Text(t)) => {
                if let Ok(text) = t.decode() {
                    if let Some(item) = &mut item {
                        append(item, &stack, &text);
                    } else if stack
                        .last()
                        .is_some_and(|s| matches!(s.as_str(), "title" | "link" | "generator"))
                    {
                        channel.push_str(&text);
                        channel.push(' ');
                    }
                }
            }
            Ok(Event::CData(t)) => {
                if let Some(item) = &mut item {
                    append(item, &stack, &String::from_utf8_lossy(&t));
                }
            }
            Ok(Event::GeneralRef(e)) => {
                if let Some(item) = &mut item {
                    let text = e
                        .resolve_char_ref()
                        .ok()
                        .flatten()
                        .map(|c| c.to_string())
                        .unwrap_or_else(|| {
                            let name = e.decode().map(|s| s.to_string()).unwrap_or_default();
                            quick_xml::escape::unescape(&format!("&{name};"))
                                .map(|s| s.into_owned())
                                .unwrap_or_default()
                        });
                    append(item, &stack, &text);
                }
            }
            Ok(Event::End(e)) => {
                if e.local_name().as_ref() == b"title" {
                    if let Some(item) = &mut item {
                        let title = std::mem::take(&mut item.title_part);
                        // RSS title and media:title can repeat the same complete value.
                        // Do not halve strings or deduplicate words within a title.
                        if item.seen_titles.insert(title.trim().to_owned()) {
                            item.title.push_str(&title);
                        }
                    }
                }
                if matches!(e.local_name().as_ref(), b"entry" | b"item") {
                    if let Some(item) = item.take() {
                        let url = if item.url.trim().is_empty() {
                            item.fallback_id.trim()
                        } else {
                            item.url.trim()
                        };
                        if network::validate_url(url).is_ok() {
                            let is_aggregator = aggregator(&channel)
                                || aggregator(&item.comments)
                                || item.text.contains("news.ycombinator.com/item");
                            let date =
                                (!item.date.trim().is_empty()).then(|| item.date.trim().to_owned());
                            out.push(Material {
                                title: item.title.trim().into(),
                                url: url.into(),
                                text: plain(&format!("<body>{}</body>", item.text)),
                                published: if is_aggregator { None } else { date.clone() },
                                discovered: if is_aggregator { date } else { None },
                                discovered_via: feed_url
                                    .map(str::to_owned)
                                    .or_else(|| is_aggregator.then(|| "Hacker News feed".into())),
                                publication_source: if is_aggregator {
                                    "aggregator_submission"
                                } else {
                                    "feed_publication"
                                },
                                publisher: network::validate_url(url)
                                    .ok()
                                    .and_then(|u| u.host_str().map(str::to_owned))
                                    .unwrap_or_default(),
                                fulltext: false,
                                pdf: None,
                            });
                        }
                    }
                    if out.len() >= 200 {
                        break;
                    }
                }
                stack.pop();
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    out
}
fn is_han(c: char) -> bool {
    ('一'..='鿿').contains(&c)
}
fn stem(word: &str) -> String {
    let word = word.to_lowercase();
    if word.len() > 5 && word.ends_with("ics") {
        return word[..word.len() - 3].into();
    }
    if word.len() > 4 && word.ends_with("ic") {
        return word[..word.len() - 2].into();
    }
    if word.len() > 5 && word.ends_with("ies") {
        return format!("{}y", &word[..word.len() - 3]);
    }
    if word.len() > 4
        && word.ends_with('s')
        && !word.ends_with("ss")
        && !word.ends_with("is")
        && !word.ends_with("us")
    {
        return word[..word.len() - 1].into();
    }
    word
}
fn meaningful_terms(queries: &[String]) -> HashSet<String> {
    const STOP:&str="a an and are as at be been being but by can could did do does for from had has have how i if in into is it its may might more most much my no not of on only or our out over own should so some such than that the their them then there these they this those through to too under up us very was we were what when where which while who why will with would you your about after before between both each other same also all any new news latest today current future market industry technology trend trends research study evidence support opposing counter risk risks analysis question pay buy demand customer customers failure failures progress development think know want learn discuss understand whether";
    let stops: HashSet<_> = STOP.split_whitespace().collect();
    let mut terms = HashSet::new();
    for query in queries {
        for word in query.split(|c: char| !c.is_ascii_alphanumeric()) {
            let word = word.to_lowercase();
            if (word.len() >= 3
                || matches!(
                    word.as_str(),
                    "ai" | "ml" | "vr" | "ar" | "ev" | "3d" | "xr" | "ui" | "ux"
                ))
                && !stops.contains(word.as_str())
                && !word.chars().all(|c| c.is_ascii_digit())
            {
                terms.insert(stem(&word));
            }
        }
        let mut chinese = query.clone();
        for filler in [
            "我想知道",
            "我想了解",
            "我觉得",
            "想知道",
            "有没有人",
            "会不会",
            "有没有",
            "为什么",
            "怎么样",
            "是什么",
            "新进展",
            "愿意付费",
            "有人买单",
            "有没有",
            "关于",
            "研究",
            "未来",
            "行业",
            "市场",
            "如何",
            "是否",
            "支持",
            "反证",
            "证据",
            "分析",
            "发展",
            "最新",
            "新闻",
            "资讯",
            "我的",
            "订阅",
            "来源",
            "请问",
            "可以",
            "可能",
            "需要",
            "值得",
            "买单",
            "付费",
            "会",
            "的",
            "了",
            "和",
            "与",
            "在",
            "对",
            "是",
            "能",
            "吗",
        ] {
            chinese = chinese.replace(filler, " ");
        }
        for run in chinese
            .split(|c| !is_han(c))
            .filter(|s| s.chars().count() >= 2)
        {
            let chars: Vec<_> = run.chars().collect();
            for size in [2, 3] {
                for window in chars.windows(size).take(24) {
                    let term: String = window.iter().collect();
                    if !matches!(
                        term.as_str(),
                        "技术"
                            | "智能"
                            | "机器"
                            | "目标"
                            | "用户"
                            | "问题"
                            | "方法"
                            | "方向"
                            | "影响"
                            | "结果"
                            | "需求"
                            | "愿意"
                            | "应用"
                            | "情况"
                            | "思考"
                            | "价值"
                    ) {
                        terms.insert(term);
                    }
                }
            }
        }
    }
    terms
}
pub(crate) fn filter_topics(items: &mut Vec<Material>, queries: &[String]) -> usize {
    if queries.iter().all(|q| q.trim().is_empty()) {
        return 0;
    }
    let before = items.len();
    items.retain(|m| relevant(m, queries));
    before - items.len()
}
pub(crate) fn relevant(m: &Material, queries: &[String]) -> bool {
    let terms = meaningful_terms(queries);
    let text = format!("{} {}", m.title, m.text).to_lowercase();
    let words: HashSet<_> = text
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(stem)
        .collect();
    terms.iter().any(|term| {
        if term.chars().any(is_han) {
            text.contains(term)
        } else {
            words.contains(term)
        }
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_duplicate_media_title_is_kept_once_without_guessing_repeated_words() {
        let xml = "<rss xmlns:media='http://search.yahoo.com/mrss/'><channel><item><title>AI &amp; AI</title><link>https://example.com/article</link><media:content><media:title><![CDATA[AI & AI]]></media:title></media:content></item></channel></rss>";
        let items = parse(xml);
        assert_eq!(items[0].title, "AI & AI");
        let distinct = xml.replace("<![CDATA[AI & AI]]>", "<![CDATA[Related video]]>");
        assert_eq!(parse(&distinct)[0].title, "AI & AIRelated video");
        let repeated = "<rss><channel><item><title>NeverNever</title><link>https://example.com/a</link></item></channel></rss>";
        assert_eq!(parse(repeated)[0].title, "NeverNever");
    }
    #[test]
    fn atom_alternate_link_overrides_urn_for_start_and_empty_tags() {
        for link in [
            "<link rel=\"alternate\" href=\"https://example.com/article\"></link>",
            "<link href=\"https://example.com/article\"/>",
        ] {
            let xml=format!("<feed xmlns=\"http://www.w3.org/2005/Atom\"><entry><id>urn:uuid:1234</id><title>A &amp; B</title>{link}<summary>Actual paper abstract.</summary></entry></feed>");
            let items = parse(&xml);
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].url, "https://example.com/article");
            assert_eq!(items[0].title, "A & B");
        }
    }
    #[test]
    fn hn_rss_channel_comments_and_origin_expose_discovery_not_publication() {
        for (channel, comments, origin) in [
            ("<title>Hacker News: Front Page</title>", "", None),
            ("<link>https://news.ycombinator.com/</link>", "", None),
            (
                "",
                "<comments>https://news.ycombinator.com/item?id=1</comments>",
                None,
            ),
            ("", "", Some("https://hnrss.org/frontpage")),
        ] {
            let xml=format!("<rss><channel>{channel}<item><title>Old video</title><link>https://youtube.com/watch?v=old</link><pubDate>Mon, 12 Oct 2026 00:00:00 GMT</pubDate>{comments}<description>Original article summary.</description></item></channel></rss>");
            let items = parse_from(&xml, origin);
            assert_eq!(items.len(), 1);
            assert!(items[0].published.is_none());
            assert!(items[0].discovered.is_some());
            assert_eq!(items[0].publication_source, "aggregator_submission");
        }
        let direct=parse("<rss><channel><item><title>Original robotics paper</title><link>https://lab.example/paper</link><pubDate>2026-10-11</pubDate></item></channel></rss>");
        assert_eq!(direct[0].published.as_deref(), Some("2026-10-11"));
        assert!(direct[0].discovered.is_none());
    }
    #[test]
    fn hn_comments_url_in_description_is_submission_metadata() {
        let xml="<rss><channel><item><title>Old essay</title><link>https://example.com/essay</link><pubDate>2026-10-12</pubDate><description>Comments URL: https://news.ycombinator.com/item?id=10</description></item></channel></rss>";
        let item = parse(xml).remove(0);
        assert!(item.published.is_none());
        assert_eq!(item.discovered.as_deref(), Some("2026-10-12"));
    }
    #[test]
    fn feeds_only_keep_subscriptions_while_interests_filter_unrelated_entries() {
        let xml="<rss><channel><item><title>Why the video will return</title><link>https://example.com/article</link><description>What the author will do to make it work.</description></item></channel></rss>";
        let mut items = parse(xml);
        assert_eq!(filter_topics(&mut items, &[]), 0);
        assert_eq!(items.len(), 1);
        assert_eq!(filter_topics(&mut items, &["robotics".into()]), 1);
        assert!(items.is_empty());
    }
}
