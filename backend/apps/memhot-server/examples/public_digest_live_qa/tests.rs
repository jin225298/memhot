use super::*;

fn fixture() -> (Connection, DateTime<Utc>, String) {
    let at = "2026-10-13T10:40:00Z".parse::<DateTime<Utc>>().unwrap();
    let key = edition_key("robotics", at);
    let conn = Connection::open_in_memory().unwrap();
    for table in ["users", "records", "devices", "jobs", "receipts", "pairs"] {
        conn.execute_batch(&format!("CREATE TABLE {table}(id TEXT)"))
            .unwrap();
    }
    conn.execute_batch("CREATE TABLE public_cache(cache_key TEXT PRIMARY KEY,kind TEXT,body TEXT,expires_at INTEGER DEFAULT 0,lease_token TEXT,lease_until INTEGER DEFAULT 0,retry_after INTEGER,last_error TEXT);
        CREATE TABLE public_editorial_inputs(edition_key TEXT PRIMARY KEY,body TEXT,fingerprint TEXT,created INTEGER,expires INTEGER);
        CREATE TABLE public_agent_runs(run_key TEXT PRIMARY KEY,data TEXT);
        CREATE TABLE public_model_calls(operation_key TEXT PRIMARY KEY,status TEXT,response TEXT,attempt INTEGER);
        CREATE TABLE public_model_attempts(operation_key TEXT,attempt INTEGER,status TEXT,PRIMARY KEY(operation_key,attempt));").unwrap();
    conn.execute("INSERT INTO public_cache(cache_key,kind,retry_after,last_error) VALUES(?,'edition',?,'editorial_review_failed')", params![key,at.timestamp()+86400]).unwrap();
    conn.execute(
        "INSERT INTO public_editorial_inputs VALUES(?,?,?,?,?)",
        params![
            key,
            "{}",
            hash(b"{}"),
            at.timestamp(),
            at.timestamp() + 86400
        ],
    )
    .unwrap();
    let stories = json!([{"id":"story-1"}]);
    let fingerprint = hash(format!("rules{stories}").as_bytes());
    let review_purpose = format!("digest-fact-review-{}", &fingerprint[..24]);
    for (purpose, mode, steps) in [
        ("digest-edit-v2", "digest", 6),
        ("digest-edit-v2-format", "review", 1),
        (review_purpose.as_str(), "review", 1),
    ] {
        let run = run_key(&key, purpose);
        let prompt = if purpose == review_purpose {
            json!({"topic":"机器人","stories":[{"id":"story-1","citedMaterials":[]}]})
        } else {
            json!({"topic":"机器人"})
        };
        let complete = if purpose == review_purpose {
            json!({"checks":[{"id":"story-1","weakestStatement":"claim","sourceLimit":"limited evidence","contradiction":"","supported":false,"relevant":true,"noUnsupportedDetail":false,"reason":"unsupported detail"}]}).to_string()
        } else {
            "finished".into()
        };
        let saved = json!({"input":{"runId":run,"mode":mode,"prompt":prompt.to_string(),"instructions":"rules"},"complete":complete,"pending_models":[],"pending_tools":[],"epoch":null,"step":steps,"context":{"profile":{},"references":[]}});
        conn.execute(
            "INSERT INTO public_agent_runs VALUES(?,?)",
            params![run, saved.to_string()],
        )
        .unwrap();
        for index in 0..steps {
            let operation = hash(format!("{run}:model:{index}").as_bytes());
            conn.execute(
                "INSERT INTO public_model_calls VALUES(?,'completed','retained response',1)",
                [&operation],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO public_model_attempts VALUES(?,1,'completed')",
                [&operation],
            )
            .unwrap();
        }
    }
    (conn, at, key)
}

#[test]
fn same_window_matches_the_real_qa_key_and_refuses_a_new_window() {
    let (_, at, key) = fixture();
    assert_eq!(
        key,
        "edition-fcd2bc6a21e26989f89ff5675ac4ce6d87d7108b64aa25489bfe9efb3fc2d154"
    );
    assert!(same_window("robotics", &key, at).is_ok());
    assert!(same_window("robotics", &key, at + chrono::Duration::hours(6)).is_err());
    assert!(same_window("robotics", &key, "2026-10-13T11:59:00Z".parse().unwrap()).is_err());
}

#[test]
fn resume_rejects_private_unresolved_or_different_scope_state() {
    let (conn, at, key) = fixture();
    assert!(validate_store(&conn, "robotics", "机器人", &key, at).is_ok());
    for change in [
        "INSERT INTO users VALUES('personal')",
        "INSERT INTO records VALUES('personal')",
        "UPDATE public_model_calls SET status='pending' WHERE operation_key=(SELECT operation_key FROM public_model_calls LIMIT 1)",
        "UPDATE public_model_calls SET status='unknown' WHERE operation_key=(SELECT operation_key FROM public_model_calls LIMIT 1)",
        "UPDATE public_model_calls SET response=NULL WHERE operation_key=(SELECT operation_key FROM public_model_calls LIMIT 1)",
        "UPDATE public_model_calls SET operation_key='another-scope' WHERE operation_key=(SELECT operation_key FROM public_model_calls LIMIT 1)",
        "UPDATE public_model_attempts SET status='unknown' WHERE operation_key=(SELECT operation_key FROM public_model_attempts LIMIT 1)",
        "UPDATE public_cache SET body='already published'",
        "UPDATE public_cache SET lease_token='active'",
        "UPDATE public_cache SET lease_until=1",
        "UPDATE public_cache SET last_error='public_outcome_unknown'",
        "UPDATE public_editorial_inputs SET fingerprint='changed'",
        "UPDATE public_editorial_inputs SET expires=0",
        "UPDATE public_agent_runs SET data=json_set(data,'$.pending_models',json('[{}]')) WHERE run_key=(SELECT run_key FROM public_agent_runs LIMIT 1)",
        "UPDATE public_agent_runs SET data=json_set(data,'$.complete',null) WHERE run_key=(SELECT run_key FROM public_agent_runs LIMIT 1)",
    ] {
        let (conn, at, key) = fixture();
        conn.execute_batch(change).unwrap();
        assert!(validate_store(&conn, "robotics", "机器人", &key, at).is_err(), "accepted unsafe change: {change}");
    }
    assert!(validate_store(&conn, "robotics", "other topic", &key, at).is_err());
}

#[test]
fn resume_requires_complete_review_fields_and_matching_story_ids() {
    for field in [
        "weakestStatement",
        "sourceLimit",
        "contradiction",
        "supported",
        "relevant",
        "noUnsupportedDetail",
        "reason",
        "id",
    ] {
        let (conn, at, key) = fixture();
        let (run, raw): (String, String) = conn.query_row("SELECT run_key,data FROM public_agent_runs WHERE json_extract(data,'$.complete') LIKE '%checks%'", [], |r| Ok((r.get(0)?,r.get(1)?))).unwrap();
        let mut saved: Value = serde_json::from_str(&raw).unwrap();
        let mut review: Value = serde_json::from_str(saved["complete"].as_str().unwrap()).unwrap();
        review["checks"][0].as_object_mut().unwrap().remove(field);
        saved["complete"] = json!(review.to_string());
        conn.execute(
            "UPDATE public_agent_runs SET data=? WHERE run_key=?",
            params![saved.to_string(), run],
        )
        .unwrap();
        assert!(
            validate_store(&conn, "robotics", "机器人", &key, at).is_err(),
            "accepted missing check field: {field}"
        );
    }
}

#[test]
fn receipt_and_backup_creation_never_overwrites_evidence() {
    let dir = std::env::temp_dir().join(format!("memhot-resume-files-{}", memhot_server::id()));
    fs::create_dir(&dir).unwrap();
    for name in [
        RECEIPT,
        "outcome.before-resume.json",
        "report.before-resume.json",
    ] {
        let path = dir.join(name);
        create_private(&path, b"original").unwrap();
        assert!(create_private(&path, b"replacement").is_err());
        assert_eq!(fs::read(&path).unwrap(), b"original");
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    fs::remove_dir_all(dir).unwrap();
}
