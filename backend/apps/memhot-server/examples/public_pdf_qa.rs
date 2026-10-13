//! Explicit read-only transport + extraction check. No accounts, model calls or publication.
use memhot_server::agent_web;
use serde_json::json;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var("MEMHOT_PUBLIC_PDF_QA").as_deref() != Ok("1") {
        return Err("set MEMHOT_PUBLIC_PDF_QA=1 to fetch two public regression sources".into());
    }
    let mut readable = 0;
    for url in [
        "https://www.axis.com/dam/public/permalink/239109/the-state-of-ai-in-video-surveillance--report-en-US_239109.pdf",
        "https://link.springer.com/content/pdf/10.1007/s44336-025-00017-w.pdf",
    ] {
        match agent_web::read(url).await {
            Ok(material) => {
                if !material.fulltext || material.pdf.is_none() || material.published.is_some() {
                    return Err("PDF identity or date boundary failed".into());
                }
                readable += 1;
                println!("{}", json!({"url":material.url,"status":"readable",
                    "characters":material.text.chars().count(),"extraction":material.extraction(),
                    "publicationDateSource":material.publication_source}));
            }
            Err(error) => println!("{}", json!({"url":url,"status":"unavailable","code":error.code})),
        }
    }
    if readable == 0 {
        return Err("no real public PDF was readable".into());
    }
    Ok(())
}
