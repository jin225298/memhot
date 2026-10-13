use super::*;

fn search_policy() -> Policy {
    Policy {
        target: Url::parse("https://degoog.org/api/search").unwrap(),
        read: false,
    }
}
fn read_policy() -> Policy {
    Policy {
        target: Url::parse("https://example.com/article").unwrap(),
        read: true,
    }
}
fn fetched(url: &str, html: &str) -> ReadMetadata {
    ReadMetadata::from_response(
        &json!({"url":url,"body":html,"contentType":"text/html; charset=utf-8"}),
    )
    .unwrap()
}
#[test]
fn rpc_http_is_scoped_to_the_selected_provider_or_exact_read_url() {
    let search = search_policy();
    for url in [
        "https://degoog.org/api/search?query=Pi",
        "https://degoog.org/api/search",
    ] {
        assert!(request(&search, &json!({"url":url,"method":"POST","body":"{}","headers":{"Content-Type":"application/json"}})).is_ok());
    }
    for url in [
        "https://degoog.org/api/llm",
        "https://degoog.org:444/api/search",
        "https://api.exa.ai/search",
        "http://127.0.0.1:8791/search",
        "https://user:secret@degoog.org/api/search",
    ] {
        assert!(
            request(&search, &json!({"url":url,"method":"GET"})).is_err(),
            "{url}"
        );
    }
    let read = read_policy();
    assert!(request(
        &read,
        &json!({"url":"https://example.com/article#section","method":"GET"})
    )
    .is_ok());
    for request_value in [
        json!({"url":"https://example.com/article","method":"POST"}),
        json!({"url":"https://example.com/other","method":"GET"}),
        json!({"url":"https://example.com/article?token=private","method":"GET"}),
        json!({"url":"https://example.com/article","method":"GET","headers":{"Authorization":"secret"}}),
        json!({"url":"https://example.com/article","method":"GET","headers":{"Cookie":"private"}}),
        json!({"url":"https://example.com/article","method":"GET","body":"secret"}),
    ] {
        assert!(request(&read, &request_value).is_err());
    }
    let local = Policy {
        target: Url::parse("http://127.0.0.1/admin").unwrap(),
        read: true,
    };
    assert!(request(&local, &json!({"url":local.target.as_str(),"method":"GET"})).is_err());
}
#[test]
fn only_operator_loopback_search_is_exempt_from_public_dns_rules() {
    assert_eq!(
        operator_endpoint("http://127.0.0.1:8793").unwrap().as_str(),
        "http://127.0.0.1:8793/search"
    );
    for raw in [
        "http://127.0.0.1",
        "http://localhost:8793",
        "http://127.0.0.1:8793/admin",
        "https://127.0.0.1:8793",
        "http://user:secret@127.0.0.1:8793",
        "http://127.0.0.1:8793/?q=private",
    ] {
        assert!(operator_endpoint(raw).is_err(), "{raw}");
    }
}
#[test]
fn github_document_canonicalization_rejects_traversal_refs_queries_and_credentials() {
    assert_eq!(
        canonical_read_url("https://github.com/org/repo/blob/v1.2.3/docs/README.md#title")
            .unwrap()
            .as_str(),
        "https://raw.githubusercontent.com/org/repo/v1.2.3/docs/README.md"
    );
    for raw in [
        "https://github.com/org/repo",
        "https://github.com/org/repo/tree/main/docs",
        "https://github.com/org/repo/blob/main/a/../secret",
        "https://github.com/org/repo/blob/main/%2e%2e/secret",
        "https://github.com/org/repo/blob/feature%2fbranch/README.md",
        "https://github.com/org/repo/blob/main/README.md?raw=1",
        "https://user:secret@github.com/org/repo/blob/main/README.md",
        "http://127.0.0.1/private",
    ] {
        assert!(canonical_read_url(raw).is_err(), "{raw}");
    }
}
#[test]
fn search_snippets_cannot_become_full_pages_and_reads_require_real_text_and_identity() {
    let source = json!({"title":"Pi","url":"https://example.com/article","text":"x".repeat(120),"fulltext":true});
    let value = json!({"sources":[source]});
    let search = materials(&value, &search_policy(), None).unwrap();
    assert!(!search[0].fulltext);
    assert_eq!(search[0].publication_source, "search_reported_publication");
    assert!(
        materials(
            &value,
            &read_policy(),
            Some(&fetched("https://example.com/article", ""))
        )
        .unwrap()[0]
            .fulltext
    );
    assert!(materials(
        &value,
        &read_policy(),
        Some(&fetched("https://example.com/other", ""))
    )
    .is_err());
    for source in [
        json!({"url":"https://example.com/article","text":"short","fulltext":true}),
        json!({"url":"https://example.com/article","text":"x".repeat(120),"fulltext":false}),
    ] {
        assert_eq!(
            materials(
                &json!({"sources":[source]}),
                &read_policy(),
                Some(&fetched("https://example.com/article", ""))
            )
            .unwrap_err()
            .code,
            "web_text_unavailable"
        );
    }
}
#[test]
fn original_page_publication_survives_text_extraction_and_ignores_worker_dates() {
    let url = "https://example.com/article";
    let value = json!({"sources":[{"title":"Article","url":url,"text":"Evidence. ".repeat(30),"fulltext":true,"publishedAt":"2099-01-01"}]});
    for html in [
        "<meta property='article:published_time' content='2026-10-11T08:00:00Z'>",
        "<script type='application/ld+json'>{\"@type\":\"NewsArticle\",\"datePublished\":\"2026-10-11T08:00:00Z\",\"dateModified\":\"2026-10-12T08:00:00Z\"}</script>",
    ] {
        let read = materials(&value, &read_policy(), Some(&fetched(url, html))).unwrap();
        assert_eq!(read[0].published.as_deref(), Some("2026-10-11T08:00:00+00:00"));
        assert_eq!(read[0].publication_source, "original_page_metadata");
        assert_eq!(read[0].evidence()["publicationDateSource"], "original_page_metadata");
    }
    for html in [
        "",
        "<meta property='article:modified_time' content='2026-10-12T08:00:00Z'>",
    ] {
        let read = materials(&value, &read_policy(), Some(&fetched(url, html))).unwrap();
        assert!(read[0].published.is_none());
        assert!(read[0].discovered.is_some());
        assert_eq!(read[0].publication_source, "unknown");
    }
    assert!(materials(&value, &search_policy(), None).unwrap()[0]
        .published
        .is_none());
    let plain = ReadMetadata::from_response(&json!({"url":url,"body":"<meta property='article:published_time' content='2099-01-01'>","contentType":"text/plain"})).unwrap();
    assert!(materials(&value, &read_policy(), Some(&plain)).unwrap()[0]
        .published
        .is_none());
}
#[tokio::test]
async fn jsonl_bounds_unterminated_and_invalid_worker_output() {
    for (bytes, code) in [
        (b"{invalid}\n".to_vec(), "web_plugin_protocol"),
        (vec![b'x'; OUTPUT_LIMIT + 1], "response_too_large"),
    ] {
        let (mut writer, reader) = tokio::io::duplex(4096);
        tokio::spawn(async move {
            let _ = writer.write_all(&bytes).await;
        });
        assert_eq!(
            frame(&mut BufReader::new(reader)).await.unwrap_err().code,
            code
        );
    }
}
#[tokio::test]
async fn provider_denial_does_not_return_private_body_or_headers() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let target = Url::parse(&format!("http://{}/search", listener.local_addr().unwrap())).unwrap();
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut buf = [0; 1024];
        let _ = stream.read(&mut buf).await;
        let _=stream.write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 14\r\nSet-Cookie: secret\r\nConnection: close\r\n\r\nprivate-secret").await;
    });
    let error = http(
        &Policy {
            target: target.clone(),
            read: false,
        },
        &json!({"url":target.as_str(),"method":"GET"}),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, "provider_auth");
    assert!(!error.message.contains("secret"));
}
#[test]
fn temporary_plugin_configuration_is_private_and_removed_on_drop() {
    let dir = Workdir::new().unwrap();
    let path = dir.0.clone();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
    std::fs::write(path.join("config.json"), b"synthetic").unwrap();
    drop(dir);
    assert!(!path.exists());
}
#[tokio::test]
#[ignore = "public network plugin acceptance; no model calls"]
async fn actual_pi_web_plugin_search_and_read_without_model_calls() {
    let found = search("pi-agent-core Mario Zechner").await.unwrap();
    assert!(!found.is_empty());
    assert!(found.iter().all(|m| !m.fulltext));
    let page =
        read("https://raw.githubusercontent.com/badlogic/pi-mono/main/packages/agent/README.md")
            .await
            .unwrap();
    assert!(page.fulltext);
    assert!(page.text.contains("Agent"));
    assert!(page.text.chars().count() >= 100);
}
#[tokio::test]
#[ignore = "public network date acceptance; no model calls"]
async fn known_36kr_old_article_uses_original_state_date_without_model_calls() {
    let page = read("https://www.36kr.com/p/3231130347355525")
        .await
        .unwrap();
    assert!(page.fulltext);
    assert_eq!(
        page.published.as_deref(),
        Some("2025-04-01T08:15:43.753+00:00")
    );
    assert_eq!(page.publication_source, "original_page_metadata");
}

#[tokio::test]
async fn child_denial_is_terminal_and_host_configuration_is_removed() {
    let fixture = Workdir::new().unwrap();
    let script = fixture.0.join("worker.mjs");
    let marker = fixture.0.join("workdir-path");
    let code = format!(
        r#"
import fs from 'node:fs';
import readline from 'node:readline';
fs.writeFileSync({}, process.env.MEMHOT_WEB_WORKDIR);
const rl=readline.createInterface({{input:process.stdin}});
for await (const line of rl) {{
 const value=JSON.parse(line);
 if(value.type==='start') {{
  if(process.env.HOME!==process.env.MEMHOT_WEB_WORKDIR || process.env.PATH || process.env.OPENAI_API_KEY) process.exit(2);
  console.log(JSON.stringify({{type:'http',id:'denied',url:'http://127.0.0.1/admin',method:'GET'}}));
 }} else {{
  console.log(JSON.stringify({{type:'complete',sources:[]}}));
  rl.close(); break;
 }}
}}
"#,
        serde_json::to_string(marker.to_str().unwrap()).unwrap()
    );
    std::fs::write(&script, code).unwrap();
    let node = std::env::var_os("MEMHOT_AGENT_NODE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/opt/homebrew/bin/node"));
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        execute(node, script, json!({"type":"start"}), search_policy()),
    )
    .await
    .unwrap();
    assert_eq!(result.unwrap_err().code, "invalid_input");
    let configuration = std::fs::read_to_string(marker).unwrap();
    assert!(!std::path::Path::new(&configuration).exists());
}

#[test]
fn pdf_binary_transport_and_extraction_metadata_preserve_host_identity_and_unknown_publication() {
    use base64::{engine::general_purpose::STANDARD, Engine};
    let url = "https://example.com/report.pdf";
    let response = document_response(network::FetchedDocument {
        url: url.into(),
        data: b"%PDF-1.4\n".to_vec(),
        content_type: "application/octet-stream".into(),
        pdf: true,
    })
    .unwrap();
    assert_eq!(response["bodyEncoding"], "base64");
    assert_eq!(
        STANDARD.decode(response["body"].as_str().unwrap()).unwrap(),
        b"%PDF-1.4\n"
    );
    assert_eq!(response["contentType"], "application/pdf");
    let host = ReadMetadata::from_response(&response).unwrap();
    let mut source = json!({"title":"PDF","url":url,"text":"Original evidence. ".repeat(30),"fulltext":true,
        "publishedAt":"2099-01-01","extraction":{"format":"pdf","pagesRead":2,"totalPages":20,"truncated":true}});
    let result = materials(
        &json!({"sources":[source.clone()]}),
        &read_policy(),
        Some(&host),
    )
    .unwrap();
    assert!(result[0].published.is_none());
    assert_eq!(result[0].publication_source, "unknown");
    assert_eq!(result[0].extraction()["truncated"], true);
    for bad in [
        json!({"format":"pdf","pagesRead":0,"totalPages":20,"truncated":true}),
        json!({"format":"pdf","pagesRead":2,"totalPages":20,"truncated":false}),
        Value::Null,
    ] {
        source["extraction"] = bad;
        assert!(materials(
            &json!({"sources":[source.clone()]}),
            &read_policy(),
            Some(&host)
        )
        .is_err());
    }
    source["extraction"] = json!({"format":"pdf","pagesRead":2,"totalPages":20,"truncated":true});
    assert!(materials(
        &json!({"sources":[source]}),
        &read_policy(),
        Some(&fetched(url, ""))
    )
    .is_err());
}

#[tokio::test]
#[ignore = "read-only public PDF probe via pinned Rust HTTP and isolated unpdf; no model calls"]
async fn bounded_public_pdf_probe_without_model_calls() {
    for url in [
        "https://www.axis.com/dam/public/permalink/239109/the-state-of-ai-in-video-surveillance--report-en-US_239109.pdf",
        "https://link.springer.com/content/pdf/10.1007/s44336-025-00017-w.pdf",
    ] {
        match read(url).await {
            Ok(material) => {
                assert!(material.fulltext);
                assert!(material.pdf.is_some());
                assert!(material.published.is_none());
                assert!(material.text.chars().count() <= 18_000);
                println!("{}", json!({"source":url,"status":"readable","url":material.url,
                    "title":material.title,"characters":material.text.chars().count(),"extraction":material.extraction(),
                    "publicationDateSource":material.publication_source,"excerpt":crate::clip(&material.text,160)}));
            }
            Err(error) => println!("{}", json!({"source":url,"status":"unavailable","code":error.code})),
        }
    }
}

#[test]
fn only_public_read_transport_size_errors_become_recoverable_source_unavailability() {
    assert_eq!(
        read_source_error(Error::upstream("response_too_large", "bounded source")).code,
        "web_text_unavailable"
    );
    for code in ["network", "timeout", "provider_auth", "web_plugin_protocol"] {
        let error = read_source_error(Error::upstream(code, "original message"));
        assert_eq!(error.code, code);
        assert_eq!(error.message, "original message");
    }
}
