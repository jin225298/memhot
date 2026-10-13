use super::*;
#[test]
fn rss_atom_empty_feeds_and_summaries_have_honest_coverage() {
    let rss = "<rss><channel><title>A &amp; B</title><description>About us</description><item><title>Article</title><link>https://example.com/post</link><description><![CDATA[<p>Summary</p>]]></description><pubDate>2026-10-13</pubDate></item></channel></rss>";
    let parsed = parse_candidate(rss, "https://example.com/feed").unwrap();
    assert_eq!(parsed["title"], "A & B");
    assert_eq!(parsed["coverage"], "summary");
    assert_eq!(parsed["items"][0]["summary"], "Summary");
    assert_eq!(
        parsed["items"][0]["publishedAt"],
        "2026-10-13T00:00:00+00:00"
    );
    let atom = "<feed xmlns='http://www.w3.org/2005/Atom'><title>Atom</title><entry><title>Paper</title><link rel='alternate' href='https://example.com/paper'/></entry></feed>";
    assert_eq!(
        parse_candidate(atom, "https://example.com/atom").unwrap()["coverage"],
        "title_only"
    );
    for empty in [
        "<rss><channel/></rss>",
        "<feed xmlns='http://www.w3.org/2005/Atom'/>",
    ] {
        assert_eq!(
            parse_candidate(empty, "https://example.com/feed").unwrap()["coverage"],
            "empty"
        );
    }
}
#[test]
fn validates_whole_feed_instead_of_accepting_partial_or_hostile_input() {
    for xml in [
        "<html><body>Login</body></html>",
        "<rss/>",
        "<rss><channel>",
        "<rss><channel/></rss><rss/>",
        "<feed><entry></feed>",
        "<!DOCTYPE rss [<!ENTITY xx SYSTEM 'file:///etc/passwd'>]><rss><channel/></rss>",
        "<feed><title>&xx;</title></feed>",
        "<feed><link href='&xx;'/></feed>",
    ] {
        assert!(metadata(xml).is_err(), "{xml}");
    }
    let partial = format!(
        "<rss><channel>{}",
        "<item><title>A</title><link>https://example.com/a</link></item>".repeat(25)
    );
    assert!(parse_feed(&partial, "https://example.com/feed").is_err());
    let too_deep = format!(
        "<feed>{}{}</feed>",
        "<x>".repeat(MAX_DEPTH),
        "</x>".repeat(MAX_DEPTH)
    );
    assert!(metadata(&too_deep).is_err());
    assert!(metadata(&" ".repeat(network::MAX_BYTES + 1)).is_err());
}
#[test]
fn discovers_relative_feeds_deduplicates_and_rejects_unsafe_destinations() {
    let html = "<html><head><link rel='alternate' type='application/rss+xml' href='../rss.xml' title='Feed'><link rel='alternate' type='application/rss+xml' href='https://example.com/rss.xml#copy'><link rel='stylesheet' type='application/rss+xml' href='/wrong'><link rel='alternate' type='application/atom+xml' href='http://127.0.0.1/feed'><link rel='alternate' type='application/rss+xml' href='/feed?api_key=hidden'></head></html>";
    let (feeds, warnings) = discover(html, "https://example.com/news/").unwrap();
    assert_eq!(
        feeds,
        vec![("https://example.com/rss.xml".into(), "Feed".into())]
    );
    assert_eq!(warnings.len(), 1);
    assert!(!format!("{feeds:?}{warnings:?}").contains("hidden"));
    let many = (0..10)
        .map(|n| format!("<link rel='alternate' type='application/rss+xml' href='/feed/{n}'>"))
        .collect::<String>();
    let (feeds, warnings) = discover(&many, "https://example.com").unwrap();
    assert_eq!(feeds.len(), MAX_FEEDS);
    assert_eq!(warnings.len(), 1);
}
#[test]
fn credentials_and_query_secrets_never_become_preview_links() {
    for raw in [
        "https://user:pass@example.com/feed",
        "https://example.com/?access_token=secret",
        "https://example.com/?api-key=secret",
        "http://169.254.169.254/feed",
        "file:///etc/passwd",
    ] {
        assert!(public_url(raw).is_err());
    }
    assert!(public_url("https://example.com/?format=rss").is_ok());
    assert!(public_url("https://example.com/?author=4&feed=rss2").is_ok());
    let feed = "<feed><entry><title>Secret</title><link href='https://example.com/post?token=hidden'/></entry></feed>";
    assert!(parse_candidate(feed, "https://example.com/feed").is_err());
    let many = format!(
        "<rss><channel>{}</channel></rss>",
        (0..5)
            .map(|n| format!(
                "<item><title>Post</title><link>https://example.com/{n}</link></item>"
            ))
            .collect::<String>()
    );
    assert_eq!(
        parse_candidate(&many, "https://example.com/feed").unwrap()["items"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
}

#[test]
fn preview_title_fits_storage_and_invalid_dates_stay_unknown() {
    let xml = format!("<rss><channel><title>{}</title><item><title>A</title><link>https://example.com/a</link><pubDate>invalid</pubDate></item></channel></rss>", "长".repeat(200));
    let result = parse_candidate(&xml, "https://example.com/feed").unwrap();
    assert_eq!(result["title"].as_str().unwrap().chars().count(), 120);
    assert!(result["items"][0]["publishedAt"].is_null());
}
