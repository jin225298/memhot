//! Manual paid smoke test. Inputs are public arXiv feed abstracts only.
//! Run with MEMHOT_PROVIDER_FILE and MEMHOT_EDITORIAL_QA_DIR explicitly set.
use memhot_server::{discovery::Material, editorial, feed, hash, network, shared_public, App};
use serde_json::{json, Value};
use std::{os::unix::fs::PermissionsExt, path::PathBuf};
fn save(dir: &std::path::Path, name: &str, body: &str) {
    let path = dir.join(name);
    std::fs::write(&path, body).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
}
#[tokio::main]
async fn main() {
    let dir = PathBuf::from(
        std::env::var("MEMHOT_EDITORIAL_QA_DIR").expect("explicit QA directory required"),
    );
    let app = App::open(dir.clone()).expect("open isolated QA store");
    let topic = &shared_public::catalog()[0];
    let input_path = dir.join("public-input.json");
    let materials: Vec<Material> = if input_path.exists() {
        let v: Vec<Value> =
            serde_json::from_str(&std::fs::read_to_string(&input_path).unwrap()).unwrap();
        v.into_iter()
            .map(|m| Material {
                title: m["title"].as_str().unwrap().into(),
                url: m["url"].as_str().unwrap().into(),
                text: m["text"].as_str().unwrap().into(),
                published: m["published"].as_str().map(str::to_owned),
                discovered: None,
                discovered_via: None,
                publication_source: "feed",
                publisher: "arXiv".into(),
                fulltext: false,
                pdf: None,
            })
            .collect()
    } else {
        let fetched = network::fetch(topic.feeds[0])
            .await
            .expect("fetch public catalog feed");
        let mut materials: Vec<_> = feed::parse_from(&fetched.text, Some(topic.feeds[0]))
            .into_iter()
            .take(8)
            .collect();
        if materials.is_empty() {
            let api = "https://export.arxiv.org/api/query?search_query=cat%3Acs.AI&start=0&max_results=8&sortBy=submittedDate&sortOrder=descending";
            let fetched = network::fetch(api)
                .await
                .expect("fetch official arXiv API fallback");
            materials = feed::parse_from(&fetched.text, Some(api))
                .into_iter()
                .take(8)
                .collect();
        }
        let rows: Vec<_> = materials
            .iter()
            .map(|m| json!({"title":m.title,"url":m.url,"text":m.text,"published":m.published}))
            .collect();
        save(
            &dir,
            "public-input.json",
            &serde_json::to_string_pretty(&rows).unwrap(),
        );
        materials
    };
    let edition = "manual-public-editorial-qa-v2";
    let result =
        editorial::compose(&app, edition, topic, &materials, serde_json::Value::Null).await;
    for phase in [
        "selection_a",
        "selection_b",
        "story_editing",
        "story_review",
    ] {
        let key = hash(serde_json::to_string(&(edition, phase)).unwrap().as_bytes());
        let row: Result<(String, Option<String>), _> = app.store.conn().unwrap().query_row(
            "SELECT status,response FROM public_model_calls WHERE operation_key=?",
            [key],
            |r| Ok((r.get(0)?, r.get(1)?)),
        );
        if let Ok((status, response)) = row {
            save(
                &dir,
                &format!("{phase}.json"),
                &json!({"status":status,"response":response}).to_string(),
            );
        }
    }
    let attempts: i64 = app
        .store
        .conn()
        .unwrap()
        .query_row("SELECT count(*) FROM public_model_attempts", [], |r| {
            r.get(0)
        })
        .unwrap();
    match result {
        Ok(body) => {
            save(
                &dir,
                "edition.json",
                &serde_json::to_string_pretty(&body).unwrap(),
            );
            save(&dir, "edition.md", body["body"].as_str().unwrap());
            println!(
                "QA completed: candidates={}, stories={}, provider_attempts={}",
                materials.len(),
                body["editorial"]["acceptedStories"],
                attempts
            );
        }
        Err(error) => {
            save(
                &dir,
                "outcome.json",
                &json!({"code":error.code,"message":error.message,"providerAttempts":attempts})
                    .to_string(),
            );
            println!(
                "QA blocked: code={}, provider_attempts={}",
                error.code, attempts
            );
            std::process::exit(1);
        }
    }
}
