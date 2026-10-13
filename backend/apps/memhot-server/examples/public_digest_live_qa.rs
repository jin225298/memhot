//! Operator-only paid end-to-end check using fixed public AI sources in an isolated store.
//! No user records are read. Requires an explicit fresh MEMHOT_REPORT_QA_DIR.
//! An existing isolated, completed eight-call semantic failure can be resumed once
//! with MEMHOT_REPORT_QA_RESUME_EDITION=<exact current-window edition key>.
//! Resume preserves paid ledgers and frozen inputs/runs; it may pay for new stages.
use chrono::{DateTime, Timelike, Utc};
use memhot_server::{hash, shared_public, App};
use rusqlite::{params, Connection, OpenFlags, TransactionBehavior};
use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    fs::{self, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

type QaResult<T> = Result<T, Box<dyn std::error::Error>>;
const MARKER: &str = "public-digest-live-qa.json";
const RECEIPT: &str = "resume-receipt.json";
const LANGUAGE: &str = "zh-CN";
const COMPLETED_CALLS: i64 = 8;

fn require(ok: bool, message: &'static str) -> QaResult<()> {
    if !ok {
        return Err(message.into());
    }
    Ok(())
}

// Mirror shared_public's public pipeline version and UTC six-hour key derivation.
// Do not reset an old window then let edition() silently create a new paid scope.
fn edition_key(topic: &str, at: DateTime<Utc>) -> String {
    let window = at
        .date_naive()
        .and_hms_opt(at.hour() / 6 * 6, 0, 0)
        .expect("valid UTC six-hour boundary")
        .and_utc();
    format!(
        "edition-{}",
        hash(
            format!(
                "{topic}:{}:{LANGUAGE}:{}",
                window.to_rfc3339(),
                shared_public::PIPELINE_VERSION
            )
            .as_bytes()
        )
    )
}

fn same_window(topic: &str, key: &str, at: DateTime<Utc>) -> QaResult<()> {
    require(
        key == edition_key(topic, at),
        "resume edition differs from the current UTC six-hour window; fixed-scope replay is unavailable",
    )?;
    require(
        at.hour() % 6 != 5 || at.minute() != 59,
        "refusing resume in the closing minute of the UTC edition window",
    )
}

fn private_file(path: &Path) -> QaResult<()> {
    let metadata = fs::symlink_metadata(path)?;
    require(
        metadata.is_file() && metadata.permissions().mode() & 0o077 == 0,
        "QA evidence must be a private regular file, not a symlink",
    )
}

fn read_evidence(path: &Path) -> QaResult<Vec<u8>> {
    private_file(path)?;
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    require(bytes.len() <= 1024 * 1024, "QA evidence exceeds size limit")?;
    Ok(bytes)
}

fn create_private(path: &Path, bytes: &[u8]) -> QaResult<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn run_key(edition: &str, purpose: &str) -> String {
    hash(format!("sdk-agent-v1:{edition}:{purpose}").as_bytes())
}

fn validate_review(saved: &Value, key: &str, edition: &str, title: &str) -> QaResult<()> {
    let mut prompt: Value = serde_json::from_str(
        saved["input"]["prompt"]
            .as_str()
            .ok_or("missing frozen review prompt")?,
    )?;
    require(
        prompt["topic"] == title,
        "frozen review topic differs from QA topic",
    )?;
    let stories = prompt["stories"]
        .as_array_mut()
        .ok_or("missing frozen review stories")?;
    require(!stories.is_empty(), "empty frozen review")?;
    let mut ids = BTreeSet::new();
    for story in stories.iter_mut() {
        let id = story["id"]
            .as_str()
            .ok_or("missing review story ID")?
            .to_owned();
        require(ids.insert(id), "duplicate frozen story ID")?;
        story
            .as_object_mut()
            .ok_or("invalid frozen story")?
            .remove("citedMaterials");
    }
    let instructions = saved["input"]["instructions"]
        .as_str()
        .ok_or("missing review instructions")?;
    let fingerprint = hash(format!("{instructions}{}", serde_json::to_string(stories)?).as_bytes());
    require(
        key == run_key(
            edition,
            &format!("digest-fact-review-{}", &fingerprint[..24]),
        ),
        "frozen fact review does not belong to this edition",
    )?;
    let review: Value =
        serde_json::from_str(saved["complete"].as_str().ok_or("missing review result")?)?;
    let checks = review["checks"]
        .as_array()
        .ok_or("missing completed review checks")?;
    let required: BTreeSet<_> = [
        "id",
        "weakestStatement",
        "sourceLimit",
        "contradiction",
        "supported",
        "relevant",
        "noUnsupportedDetail",
        "reason",
    ]
    .into_iter()
    .collect();
    let mut checked = BTreeSet::new();
    require(
        review.as_object().is_some_and(|o| o.len() == 1),
        "unexpected review fields",
    )?;
    for check in checks {
        let fields: BTreeSet<_> = check
            .as_object()
            .ok_or("invalid review check")?
            .keys()
            .map(String::as_str)
            .collect();
        require(
            fields == required,
            "completed fact review contract is incomplete",
        )?;
        let id = check["id"]
            .as_str()
            .ok_or("invalid review check ID")?
            .to_owned();
        require(checked.insert(id), "duplicate completed review check ID")?;
        for field in ["weakestStatement", "sourceLimit", "reason"] {
            require(
                check[field]
                    .as_str()
                    .is_some_and(|s| !s.trim().is_empty() && s.chars().count() <= 2400),
                "invalid completed review text",
            )?;
        }
        require(
            check["contradiction"]
                .as_str()
                .is_some_and(|s| s.chars().count() <= 2400),
            "invalid completed review contradiction",
        )?;
        for field in ["supported", "relevant", "noUnsupportedDetail"] {
            require(
                check[field].is_boolean(),
                "invalid completed review verdict",
            )?;
        }
    }
    require(
        ids == checked,
        "completed review IDs differ from frozen stories",
    )
}

fn validate_store(
    conn: &Connection,
    topic: &str,
    title: &str,
    key: &str,
    at: DateTime<Utc>,
) -> QaResult<i64> {
    same_window(topic, key, at)?;
    for table in ["users", "records", "devices", "jobs", "receipts", "pairs"] {
        let count: i64 =
            conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))?;
        require(
            count == 0,
            "resume is only permitted in an isolated QA store without personal data",
        )?;
    }
    let edition_count: i64 = conn.query_row(
        "SELECT count(*) FROM public_cache WHERE kind='edition'",
        [],
        |r| r.get(0),
    )?;
    require(
        edition_count == 1,
        "QA store must contain exactly one publication scope",
    )?;
    let retry_after: i64 = conn.query_row(
        "SELECT retry_after FROM public_cache WHERE cache_key=? AND kind='edition' AND body IS NULL AND expires_at=0 AND lease_token IS NULL AND lease_until=0 AND last_error='editorial_review_failed' AND retry_after>?",
        params![key, at.timestamp()], |r| r.get(0),
    )?;
    let input_count: i64 =
        conn.query_row("SELECT count(*) FROM public_editorial_inputs", [], |r| {
            r.get(0)
        })?;
    require(
        input_count == 1,
        "QA store must contain exactly one frozen input",
    )?;
    let (body, fingerprint, created, expires): (String, String, i64, i64) = conn.query_row(
        "SELECT body,fingerprint,created,expires FROM public_editorial_inputs WHERE edition_key=?",
        [key],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )?;
    let created = DateTime::from_timestamp(created, 0).ok_or("invalid frozen input timestamp")?;
    require(
        hash(body.as_bytes()) == fingerprint
            && expires > at.timestamp()
            && edition_key(topic, created) == key,
        "frozen input changed, expired, or belongs to another scope",
    )?;
    let _: Value = serde_json::from_str(&body)?;

    // Recover the exact paid operation IDs from immutable, finished agent frames.
    // No generic 'completed' row from a different edition can authorize this resume.
    let draft = run_key(key, "digest-edit-v2");
    let format = run_key(key, "digest-edit-v2-format");
    let mut expected_calls = BTreeSet::new();
    let mut runs = conn.prepare("SELECT run_key,data FROM public_agent_runs ORDER BY run_key")?;
    let rows = runs.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    let mut run_count = 0;
    let mut saw_draft = false;
    let mut saw_format = false;
    for row in rows {
        let (run, raw) = row?;
        require(
            raw.len() <= 8 * 1024 * 1024,
            "frozen QA run exceeds size limit",
        )?;
        let saved: Value = serde_json::from_str(&raw)?;
        require(
            saved["complete"]
                .as_str()
                .is_some_and(|s| !s.trim().is_empty())
                && saved["pending_models"]
                    .as_array()
                    .is_some_and(Vec::is_empty)
                && saved["pending_tools"].as_array().is_some_and(Vec::is_empty)
                && saved["epoch"].is_null()
                && saved["input"]["runId"] == run
                && saved["context"]["profile"]
                    .as_object()
                    .is_some_and(serde_json::Map::is_empty)
                && saved["context"]["references"]
                    .as_array()
                    .is_some_and(Vec::is_empty),
            "QA agent run is unfinished, private, or inconsistent",
        )?;
        let count = if run == draft {
            saw_draft = true;
            let prompt: Value = serde_json::from_str(
                saved["input"]["prompt"]
                    .as_str()
                    .ok_or("missing draft prompt")?,
            )?;
            require(
                saved["input"]["mode"] == "digest" && prompt["topic"] == title,
                "frozen draft topic differs from QA topic",
            )?;
            6
        } else if run == format {
            saw_format = true;
            require(
                saved["input"]["mode"] == "review",
                "invalid frozen format repair",
            )?;
            1
        } else {
            require(
                saved["input"]["mode"] == "review",
                "unexpected QA agent run",
            )?;
            validate_review(&saved, &run, key, title)?;
            1
        };
        require(
            saved["step"].as_u64() == Some(count),
            "unexpected completed QA call layout",
        )?;
        for index in 0..count {
            expected_calls.insert(hash(format!("{run}:model:{index}").as_bytes()));
        }
        run_count += 1;
    }
    require(
        run_count == 3
            && saw_draft
            && saw_format
            && expected_calls.len() == COMPLETED_CALLS as usize,
        "resume requires the completed six-draft/one-format/one-review QA checkpoint",
    )?;
    let mut calls = conn.prepare("SELECT operation_key FROM public_model_calls WHERE status='completed' AND response IS NOT NULL AND length(response)>0 AND attempt=1")?;
    let completed: BTreeSet<String> = calls
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    let count: i64 = conn.query_row("SELECT count(*) FROM public_model_calls", [], |r| r.get(0))?;
    require(
        count == COMPLETED_CALLS && completed == expected_calls,
        "QA model calls are unknown, pending, missing, or outside this edition",
    )?;
    let attempts: i64 = conn.query_row("SELECT count(*) FROM public_model_attempts", [], |r| {
        r.get(0)
    })?;
    let invalid: i64 = conn.query_row("SELECT count(*) FROM public_model_attempts a LEFT JOIN public_model_calls c ON c.operation_key=a.operation_key WHERE a.status!='completed' OR a.attempt!=1 OR c.operation_key IS NULL OR c.attempt!=a.attempt", [], |r| r.get(0))?;
    require(
        attempts == COMPLETED_CALLS && invalid == 0,
        "QA paid attempt ledger is incomplete or unresolved",
    )?;
    Ok(retry_after)
}

fn prepare_resume(directory: &Path, topic: &str, title: &str, key: &str) -> QaResult<()> {
    let metadata = fs::symlink_metadata(directory)?;
    require(
        metadata.is_dir() && metadata.permissions().mode() & 0o077 == 0,
        "resume requires a private regular QA directory",
    )?;
    require(
        topic != "ai" || !memhot_server::aihot::enabled(),
        "syndicated AI editions do not support this QA resume mode",
    )?;
    same_window(topic, key, Utc::now())?;
    let outcome_bytes = read_evidence(&directory.join("outcome.json"))?;
    let report_bytes = read_evidence(&directory.join("report.json"))?;
    let outcome: Value = serde_json::from_slice(&outcome_bytes)?;
    let report: Value = serde_json::from_slice(&report_bytes)?;
    require(
        outcome["topic"] == topic
            && outcome["cacheStatus"] == "unavailable"
            && outcome["hasReport"] == false
            && outcome["modelAttempts"] == COMPLETED_CALLS
            && report.is_null(),
        "existing QA outcome/topic does not describe a completed unpublished failure",
    )?;
    let marker_path = directory.join(MARKER);
    let provenance = if marker_path.try_exists()? {
        let marker: Value = serde_json::from_slice(&read_evidence(&marker_path)?)?;
        require(
            marker["kind"] == "public_digest_live_qa"
                && marker["schemaVersion"] == 1
                && marker["topic"] == topic
                && marker["language"] == LANGUAGE
                && marker["editionKey"] == key
                && marker["pipelineVersion"] == shared_public::PIPELINE_VERSION,
            "QA marker differs from the selected topic, edition, or pipeline",
        )?;
        "marker-and-outcome"
    } else {
        // The original example predates markers. Its outcome is accepted only with
        // all the edition-bound frozen frames and exact eight-call ledger below.
        require(
            outcome.get("editionKey").is_none(),
            "missing marker for a newer QA outcome",
        )?;
        "legacy-outcome-and-frozen-scope"
    };
    private_file(&directory.join("memhot.sqlite"))?;
    let mut conn = Connection::open_with_flags(
        directory.join("memhot.sqlite"),
        OpenFlags::SQLITE_OPEN_READ_WRITE,
    )?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let retry_after = validate_store(&tx, topic, title, key, Utc::now())?;
    let receipt = json!({"kind":"public_digest_live_qa_resume","schemaVersion":1,"topic":topic,
        "editionKey":key,"pipelineVersion":shared_public::PIPELINE_VERSION,"createdAt":memhot_server::now(),
        "provenance":provenance,"previousRetryAfter":retry_after,"completedCalls":COMPLETED_CALLS,
        "outcomeSha256":hash(&outcome_bytes),"reportSha256":hash(&report_bytes),
        "action":"clear only the selected publication retry_after; preserve every paid ledger and checkpoint"});
    // Any interrupted/failed preparation consumes this one-shot authorization.
    // Never overwrite receipts or evidence, and never silently retry preparation.
    create_private(
        &directory.join(RECEIPT),
        &serde_json::to_vec_pretty(&receipt)?,
    )?;
    create_private(
        &directory.join("outcome.before-resume.json"),
        &outcome_bytes,
    )?;
    create_private(&directory.join("report.before-resume.json"), &report_bytes)?;
    fs::File::open(directory)?.sync_all()?;
    same_window(topic, key, Utc::now())?;
    let changed = tx.execute("UPDATE public_cache SET retry_after=0 WHERE cache_key=? AND kind='edition' AND body IS NULL AND lease_token IS NULL AND lease_until=0 AND last_error='editorial_review_failed' AND retry_after=?", params![key, retry_after])?;
    require(
        changed == 1,
        "publication failure changed during resume preparation",
    )?;
    tx.commit()?;
    Ok(())
}

#[tokio::main]
async fn main() -> QaResult<()> {
    let directory = PathBuf::from(std::env::var("MEMHOT_REPORT_QA_DIR")?);
    let topic = std::env::var("MEMHOT_REPORT_QA_TOPIC").unwrap_or_else(|_| "ai".into());
    let title = shared_public::catalog()
        .iter()
        .find(|t| t.id == topic)
        .ok_or("unknown public QA topic")?
        .title;
    let resume = std::env::var("MEMHOT_REPORT_QA_RESUME_EDITION").ok();
    let scope_key = resume
        .clone()
        .unwrap_or_else(|| edition_key(&topic, Utc::now()));
    require(directory.is_absolute(), "choose an absolute QA directory")?;
    if directory.try_exists()? {
        let key = resume
            .as_deref()
            .ok_or("existing QA directory requires MEMHOT_REPORT_QA_RESUME_EDITION")?;
        prepare_resume(&directory, &topic, title, key)?;
    } else {
        require(
            resume.is_none(),
            "resume is only allowed for an existing isolated QA directory",
        )?;
        fs::create_dir(&directory)?;
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
        let marker = json!({"kind":"public_digest_live_qa","schemaVersion":1,"topic":topic,
            "language":LANGUAGE,"editionKey":scope_key,
            "pipelineVersion":shared_public::PIPELINE_VERSION,"createdAt":memhot_server::now()});
        create_private(
            &directory.join(MARKER),
            &serde_json::to_vec_pretty(&marker)?,
        )?;
    }
    let app = App::open(directory.clone())?;
    same_window(&topic, &scope_key, Utc::now())?;
    let result = shared_public::edition(&app, &topic, LANGUAGE).await?;
    let attempts: i64 =
        app.store
            .conn()?
            .query_row("SELECT count(*) FROM public_model_attempts", [], |row| {
                row.get(0)
            })?;
    let report = result.edition;
    let outcome = json!({"topic":topic,"editionKey":scope_key,"pipelineVersion":shared_public::PIPELINE_VERSION,"cacheStatus":result.cache_status,"warning":result.warning,
        "modelAttempts":attempts,"hasReport":report.is_some(),
        "stories":report.as_ref().map(|r|r["editorial"]["acceptedStories"].clone()),
        "overviewStatus":report.as_ref().map(|r|r["editorial"]["overviewStatus"].clone()),
        "sourceCount":report.as_ref().and_then(|r|r["sources"].as_array()).map(Vec::len),
        "origin":report.as_ref().map(|r|r["contentOrigin"].clone()),
        "citationReview":report.as_ref().map(|r|r["editorial"]["citationReview"].clone())});
    for (name, value) in [
        ("outcome.json", outcome.clone()),
        ("report.json", json!(report)),
    ] {
        let path = directory.join(name);
        std::fs::write(&path, serde_json::to_string_pretty(&value)?)?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    println!("{outcome}");
    if report.is_none() {
        return Err("no verified report; inspect private QA evidence".into());
    }
    Ok(())
}

#[cfg(test)]
#[path = "public_digest_live_qa/tests.rs"]
mod tests;
