use super::*;
use std::collections::HashSet;
const PURPOSE: &str = "candidate-test";
struct Fixture {
    app: App,
    user: String,
    other: String,
}
impl Fixture {
    fn new() -> Self {
        let app =
            App::open(std::env::temp_dir().join(format!("memhot-candidates-{}", crate::id())))
                .unwrap();
        let (user, _) = crate::auth::create_pair(&app, "Candidate test", None).unwrap();
        let (other, _) = crate::auth::create_pair(&app, "Other tenant", None).unwrap();
        Self { app, user, other }
    }
    fn scope(&self) -> Scope {
        Scope::private(&self.app, &self.user, None, "same-run-id").unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(self.app.data_dir.as_ref());
    }
}
fn material(n: usize) -> Material {
    Material {
        url: format!("https://example.com/article/{n}"),
        title: format!("Candidate {n}"),
        text: format!("Evidence {n}: {}", "a".repeat(500)),
        published: Some("2026-10-13".into()),
        discovered: None,
        discovered_via: Some("public-search".into()),
        publication_source: "search_reported_publication",
        publisher: "example.com".into(),
        fulltext: false,
        pdf: None,
    }
}
#[test]
fn every_candidate_remains_pageable_beyond_two_hundred_without_large_frames() {
    let f = Fixture::new();
    let scope = f.scope();
    store(
        &f.app,
        &scope,
        PURPOSE,
        &(0..251).map(material).collect::<Vec<_>>(),
    )
    .unwrap();
    let mut args = json!({});
    let mut seen = HashSet::new();
    let mut pages = 0;
    loop {
        let result = page(&f.app, &scope, PURPOSE, &args).unwrap();
        assert_eq!(result["totalCandidates"], 251);
        let sources = result["sources"].as_array().unwrap();
        assert!(sources.len() <= PAGE_SIZE);
        assert!(result.to_string().len() < 30000);
        for source in sources {
            assert!(seen.insert(source["sourceId"].as_str().unwrap().to_owned()));
            assert!(source["excerpt"].as_str().unwrap().chars().count() <= 320);
            assert!(source.get("text").is_none());
            assert_eq!(source["candidateOnly"], true);
            let stored = lookup(
                &f.app,
                &scope,
                PURPOSE,
                source["sourceId"].as_str().unwrap(),
            )
            .unwrap();
            assert!(stored.text.len() > 320);
        }
        pages += 1;
        if result["hasMore"] == false {
            assert!(result["nextCursor"].is_null());
            break;
        }
        args = json!({"cursor":result["nextCursor"]});
    }
    assert_eq!(pages, 13);
    assert_eq!(seen.len(), 251);
    let filtered = page(&f.app, &scope, PURPOSE, &json!({"query":"Candidate 250"})).unwrap();
    assert_eq!(filtered["totalCandidates"], 1);
    assert_eq!(filtered["sources"][0]["title"], "Candidate 250");
}
#[test]
fn audience_run_and_query_are_bound_to_cursors_and_lookups() {
    let f = Fixture::new();
    let scope = f.scope();
    store(
        &f.app,
        &scope,
        PURPOSE,
        &(0..41).map(material).collect::<Vec<_>>(),
    )
    .unwrap();
    let first = page(&f.app, &scope, PURPOSE, &json!({"query":"Candidate"})).unwrap();
    let id = first["sources"][0]["sourceId"].as_str().unwrap();
    let cursor = json!({"cursor":first["nextCursor"]});
    let other = Scope::private(&f.app, &f.other, None, "same-run-id").unwrap();
    let run = Scope::private(&f.app, &f.user, None, "another-run").unwrap();
    let public = Scope::public("same-run-id", "ai").unwrap();
    for different in [&other, &run, &public] {
        assert!(lookup(&f.app, different, PURPOSE, id).is_err());
        assert!(page(&f.app, different, PURPOSE, &cursor).is_err());
        assert_eq!(
            page(&f.app, different, PURPOSE, &json!({})).unwrap()["totalCandidates"],
            0
        );
    }
    assert!(lookup(&f.app, &scope, "other-purpose", id).is_err());
    assert!(page(&f.app, &scope, "other-purpose", &cursor).is_err());
    assert!(page(
        &f.app,
        &scope,
        PURPOSE,
        &json!({"cursor":first["nextCursor"],"query":"different"})
    )
    .is_err());
    assert!(page(&f.app, &scope, PURPOSE, &json!({"cursor":"a".repeat(40)})).is_err());
    assert_eq!(
        page(&f.app, &scope, PURPOSE, &json!({"query":"%' OR 1=1 --"})).unwrap()["totalCandidates"],
        0
    );
    assert!(page(&f.app, &scope, PURPOSE, &json!({"offset":20})).is_err());
    assert!(page(&f.app, &scope, PURPOSE, &json!({"query":"a".repeat(181)})).is_err());
}
#[test]
fn canonical_duplicates_keep_original_cited_identity_and_public_catalogs_stay_separate() {
    let f = Fixture::new();
    let scope = Scope::public("edition", "ai").unwrap();
    let other = Scope::public("edition", "robotics").unwrap();
    let mut initial = material(0);
    initial.url.push_str("?utm_source=feed");
    let id = initial.evidence()["id"].as_str().unwrap().to_owned();
    store(&f.app, &scope, PURPOSE, &[initial.clone(), material(0)]).unwrap();
    let result = page(&f.app, &scope, PURPOSE, &json!({})).unwrap();
    assert_eq!(result["totalCandidates"], 1);
    assert_eq!(result["sources"][0]["sourceId"], id);
    assert_eq!(
        lookup(&f.app, &scope, PURPOSE, &id).unwrap().url,
        initial.url
    );
    assert!(lookup(&f.app, &other, PURPOSE, &id).is_err());
    let mut unsafe_url = material(2);
    unsafe_url.url = "http://127.0.0.1/private".into();
    store(&f.app, &scope, PURPOSE, &[unsafe_url]).unwrap();
    assert_eq!(
        page(&f.app, &scope, PURPOSE, &json!({})).unwrap()["totalCandidates"],
        1
    );
}
#[test]
fn keyset_snapshot_does_not_include_later_insertions_or_skip_after_expiry() {
    let f = Fixture::new();
    let scope = f.scope();
    store(
        &f.app,
        &scope,
        PURPOSE,
        &(0..41).map(material).collect::<Vec<_>>(),
    )
    .unwrap();
    let first = page(&f.app, &scope, PURPOSE, &json!({})).unwrap();
    store(&f.app, &scope, PURPOSE, &[material(1000)]).unwrap();
    // Deleting an earlier row must not shift an offset and skip a later result.
    let id = first["sources"][0]["sourceId"].as_str().unwrap();
    f.app
        .store
        .conn()
        .unwrap()
        .execute(
            "UPDATE agent_candidates SET expires_at=0 WHERE source_id=?",
            [id],
        )
        .unwrap();
    let second = page(
        &f.app,
        &scope,
        PURPOSE,
        &json!({"cursor":first["nextCursor"]}),
    )
    .unwrap();
    assert_eq!(second["sources"][0]["title"], "Candidate 20");
    assert_eq!(second["totalCandidates"], 40);
    let final_page = page(
        &f.app,
        &scope,
        PURPOSE,
        &json!({"cursor":second["nextCursor"]}),
    )
    .unwrap();
    assert_eq!(final_page["sources"].as_array().unwrap().len(), 1);
    assert_eq!(final_page["sources"][0]["title"], "Candidate 40");
    assert_eq!(final_page["hasMore"], false);
}
#[test]
fn expiry_and_account_clear_remove_pool_and_reject_stale_epoch() {
    let f = Fixture::new();
    let scope = f.scope();
    store(
        &f.app,
        &scope,
        PURPOSE,
        &(0..21).map(material).collect::<Vec<_>>(),
    )
    .unwrap();
    let first = page(&f.app, &scope, PURPOSE, &json!({})).unwrap();
    let id = first["sources"][0]["sourceId"].as_str().unwrap();
    retention(&f.app, chrono::Utc::now().timestamp() + TTL_SECONDS).unwrap();
    assert!(lookup(&f.app, &scope, PURPOSE, id).is_err());
    assert!(page(
        &f.app,
        &scope,
        PURPOSE,
        &json!({"cursor":first["nextCursor"]})
    )
    .is_err());
    store(&f.app, &scope, PURPOSE, &[material(30)]).unwrap();
    {
        let mut conn = f.app.store.conn().unwrap();
        let tx = conn.transaction().unwrap();
        clear_user(&tx, &f.user).unwrap();
        tx.execute("UPDATE users SET epoch=epoch+1 WHERE id=?", [&f.user])
            .unwrap();
        tx.commit().unwrap();
    }
    assert!(store(&f.app, &scope, PURPOSE, &[material(31)]).is_err());
    assert!(page(&f.app, &scope, PURPOSE, &json!({})).is_err());
    let fresh = f.scope();
    assert_eq!(
        page(&f.app, &fresh, PURPOSE, &json!({})).unwrap()["totalCandidates"],
        0
    );
    let rows: i64 = f
        .app
        .store
        .conn()
        .unwrap()
        .query_row(
            "SELECT count(*) FROM agent_candidates WHERE user_id=?",
            [&f.user],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(rows, 0);
}

#[test]
fn search_collection_returns_new_batch_after_large_seed_and_keeps_duplicate_ids() {
    let f = Fixture::new();
    let scope = f.scope();
    let mut seeds = (0..251).map(material).collect::<Vec<_>>();
    seeds[0].url.push_str("?utm_source=initial");
    let existing_id = seeds[0].evidence()["id"].as_str().unwrap().to_owned();
    store(&f.app, &scope, PURPOSE, &seeds).unwrap();
    let fresh = store_and_page(
        &f.app,
        &scope,
        PURPOSE,
        &(1000..1025).map(material).collect::<Vec<_>>(),
    )
    .unwrap();
    assert_eq!(fresh["sources"][0]["title"], "Candidate 1000");
    assert_eq!(fresh["totalCandidates"], 25);
    let second = page(
        &f.app,
        &scope,
        PURPOSE,
        &json!({"cursor":fresh["nextCursor"]}),
    )
    .unwrap();
    assert_eq!(second["collectionId"], fresh["collectionId"]);
    assert_eq!(second["sources"].as_array().unwrap().len(), 5);
    assert_eq!(second["sources"][4]["title"], "Candidate 1024");
    assert_eq!(second["hasMore"], false);
    let mixed = store_and_page(
        &f.app,
        &scope,
        PURPOSE,
        &[material(0), material(2000), material(0)],
    )
    .unwrap();
    assert_eq!(mixed["totalCandidates"], 2);
    assert_eq!(mixed["sources"][0]["sourceId"], existing_id);
    assert_eq!(mixed["sources"][1]["title"], "Candidate 2000");
    assert_eq!(
        page(&f.app, &scope, PURPOSE, &json!({})).unwrap()["totalCandidates"],
        277
    );
    let filter = page(
        &f.app,
        &scope,
        PURPOSE,
        &json!({"collectionId":fresh["collectionId"], "query":"Candidate 1024"}),
    )
    .unwrap();
    assert_eq!(filter["totalCandidates"], 1);
    let other = Scope::private(&f.app, &f.other, None, "same-run-id").unwrap();
    assert!(page(
        &f.app,
        &other,
        PURPOSE,
        &json!({"collectionId":fresh["collectionId"]})
    )
    .is_err());
    assert!(page(
        &f.app,
        &scope,
        "different-purpose",
        &json!({"collectionId":fresh["collectionId"]})
    )
    .is_err());
    assert!(page(
        &f.app,
        &scope,
        PURPOSE,
        &json!({"cursor":fresh["nextCursor"],"collectionId":mixed["collectionId"]})
    )
    .is_err());
    assert!(page(
        &f.app,
        &scope,
        PURPOSE,
        &json!({"collectionId":"a".repeat(40)})
    )
    .is_err());
    let empty = store_and_page(&f.app, &scope, PURPOSE, &[]).unwrap();
    assert_eq!(empty["totalCandidates"], 0);
    assert!(empty["sources"].as_array().unwrap().is_empty());
    assert_eq!(empty["hasMore"], false);
}

#[test]
fn collection_expiry_and_clear_cascade_members_and_cursors() {
    let f = Fixture::new();
    let scope = f.scope();
    let result = store_and_page(
        &f.app,
        &scope,
        PURPOSE,
        &(0..21).map(material).collect::<Vec<_>>(),
    )
    .unwrap();
    retention(&f.app, chrono::Utc::now().timestamp() + TTL_SECONDS).unwrap();
    assert!(page(
        &f.app,
        &scope,
        PURPOSE,
        &json!({"collectionId":result["collectionId"]})
    )
    .is_err());
    assert!(page(
        &f.app,
        &scope,
        PURPOSE,
        &json!({"cursor":result["nextCursor"]})
    )
    .is_err());
    store_and_page(
        &f.app,
        &scope,
        PURPOSE,
        &(0..21).map(material).collect::<Vec<_>>(),
    )
    .unwrap();
    let conn = f.app.store.conn().unwrap();
    clear_user(&conn, &f.user).unwrap();
    for table in [
        "agent_candidates",
        "agent_candidate_collections",
        "agent_candidate_members",
        "agent_candidate_cursors",
    ] {
        let count: i64 = conn
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0, "{table}");
    }
}

#[test]
fn old_cursor_schema_migrates_without_discarding_inventory() {
    let f = Fixture::new();
    {
        let conn = f.app.store.conn().unwrap();
        conn.execute_batch("CREATE TABLE agent_candidate_cursors(token TEXT PRIMARY KEY,audience TEXT NOT NULL,run_key TEXT NOT NULL,user_id TEXT REFERENCES users(id) ON DELETE CASCADE,query TEXT NOT NULL,after_sequence INTEGER NOT NULL,max_sequence INTEGER NOT NULL,expires_at INTEGER NOT NULL);").unwrap();
        migrate(&conn).unwrap();
        migrate(&conn).unwrap();
        let added: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info('agent_candidate_cursors') WHERE name='collection_id')", [], |r|r.get(0)).unwrap();
        assert!(added);
    }
    let result = store_and_page(&f.app, &f.scope(), PURPOSE, &[material(0)]).unwrap();
    assert_eq!(result["totalCandidates"], 1);
}
