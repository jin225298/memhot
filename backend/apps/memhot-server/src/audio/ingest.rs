//! Raw audio exists only in this request's bounded memory. Durable receipts own replay.
use super::{asr_units, parse_wav, transcribe, AudioAdmission};
use crate::{
    auth::User,
    error::{Error, Result},
    hash, id, ledger, models, now, App,
};
use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    Json,
};
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::HashSet,
    sync::{Mutex, OnceLock},
    time::Duration,
};

type Response = (StatusCode, Json<Value>);

fn active_requests() -> &'static Mutex<HashSet<String>> {
    static ACTIVE: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    ACTIVE.get_or_init(|| Mutex::new(HashSet::new()))
}
struct ActiveRequest(String);
impl ActiveRequest {
    fn begin(app: &App, user: &str, source: &str) -> Result<Self> {
        let key = format!("{}:{user}:{source}", app.data_dir.display());
        if !active_requests()
            .lock()
            .map_err(|_| Error::internal())?
            .insert(key.clone())
        {
            return Err(Error::new(
                StatusCode::CONFLICT,
                "audio_processing",
                "该片段仍在转写，请稍后重试",
            ));
        }
        Ok(Self(key))
    }
}
impl Drop for ActiveRequest {
    fn drop(&mut self) {
        if let Ok(mut active) = active_requests().lock() {
            active.remove(&self.0);
        }
    }
}

pub async fn upload(
    State(app): State<App>,
    AudioAdmission { user, _permit }: AudioAdmission,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response> {
    if headers
        .get("content-type")
        .and_then(|h| h.to_str().ok())
        .is_none_or(|h| !h.starts_with("audio/wav"))
    {
        return Err(Error::bad("音频Content-Type须为audio/wav"));
    }
    let audio = parse_wav(&body)?;
    let client =
        header(&headers, "idempotency-key").ok_or_else(|| Error::bad("缺少有效Idempotency-Key"))?;
    validate_client(client)?;
    let captured = header(&headers, "x-captured-at")
        .map(str::to_owned)
        .unwrap_or_else(now);
    chrono::DateTime::parse_from_rfc3339(&captured).map_err(|_| Error::bad("录音时间无效"))?;
    let checksum = hash(&body);
    let (source, epoch) = begin(&app, &user, client, &checksum, &captured, "cloud")?;
    if terminal(&source) {
        return Ok(response(&source));
    }
    let source_id = source["id"].as_str().ok_or_else(Error::internal)?;
    let _active = ActiveRequest::begin(&app, &user, source_id)?;
    // The earlier request may have committed between begin and acquiring this process guard.
    let source = app.store.get(&user, "source", source_id)?;
    if terminal(&source) {
        return Ok(response(&source));
    }
    if let Some(file) = source["_audioPath"].as_str() {
        if file.contains('/') || file.contains('\\') {
            return Err(Error::internal());
        }
        super::LegacyAudioCleanup(app.data_dir.join("audio").join(file)).delete()?;
        if header(&headers, "x-accept-duplicate-charge") != Some("true") {
            if let Some(text) = legacy_result(
                &app,
                &user,
                source["id"].as_str().ok_or_else(Error::internal)?,
            )? {
                return complete(&app, &user, source, epoch, &text, None);
            }
        }
    }
    // Deterministic zero / near-digital-silence guard also protects non-native clients.
    // Human-speech classification is performed by Silero on the Android device.
    if audio
        .pcm
        .chunks_exact(2)
        .all(|b| i16::from_le_bytes([b[0], b[1]]).unsigned_abs() < 8)
    {
        return complete(&app, &user, source, epoch, "", None);
    }
    let config = models::config(&app, &user, "asr")?;
    if !models::configured(&config) {
        return Err(Error::new(
            StatusCode::CONFLICT,
            "model_unconfigured",
            "请配置ASR模型后重试转写",
        ));
    }
    let source_id = source["id"].as_str().ok_or_else(Error::internal)?;
    let retry = header(&headers, "x-audio-retry-key");
    let key = if let Some(retry) = retry {
        if header(&headers, "x-accept-duplicate-charge") != Some("true")
            || retry.len() > 80
            || retry.is_empty()
            || !retry
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        {
            return Err(Error::bad("重试须明确确认可能重复收费并使用新的重试ID"));
        }
        format!("audio:{source_id}:retry:{retry}")
    } else {
        format!("audio:{source_id}")
    };
    let text = if let Some(text) =
        ledger::reserve_fingerprinted(&app, &user, None, &key, &checksum, "asr", asr_units(&body)?)?
    {
        text
    } else {
        // An abandoned HTTP request leaves a pending receipt. Retrying never repeats that paid call.
        let result = tokio::time::timeout(Duration::from_secs(100), transcribe(&config, &body))
            .await
            .unwrap_or_else(|_| {
                Err(Error::upstream(
                    "timeout",
                    "音频转写超时；本机保留录音，结果不明时不会自动再次付费",
                ))
            });
        ledger::settle_request(&app, &user, &key, &result)?;
        match result {
            Ok(text) => text,
            Err(error) => {
                mark_awaiting(&app, &user, source_id, epoch, &error)?;
                return Err(error);
            }
        }
    };
    complete(&app, &user, source, epoch, &text, Some(&key))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LocalTranscript {
    client_id: String,
    text: String,
    captured_at: String,
    duration_ms: u64,
    model_id: String,
    model_version: String,
    audio_hash: String,
}
pub async fn upload_transcript(
    State(app): State<App>,
    User(user): User,
    Json(input): Json<LocalTranscript>,
) -> Result<Response> {
    validate_client(&input.client_id)?;
    if input.text.len() > 40_000
        || !(1..=60_000).contains(&input.duration_ms)
        || input.model_id != "sensevoice-small-int8"
        || input.model_version != "2024-07-17"
        || input.audio_hash.len() != 64
        || !input.audio_hash.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(Error::bad("离线转写参数无效"));
    }
    chrono::DateTime::parse_from_rfc3339(&input.captured_at)
        .map_err(|_| Error::bad("录音时间无效"))?;
    let checksum = hash(
        format!(
            "{}\0{}\0{}\0{}\0{}\0{}",
            input.audio_hash,
            input.text,
            input.model_id,
            input.model_version,
            input.captured_at,
            input.duration_ms
        )
        .as_bytes(),
    );
    let (mut source, epoch) = begin(
        &app,
        &user,
        &input.client_id,
        &checksum,
        &input.captured_at,
        "local",
    )?;
    if terminal(&source) {
        return Ok(response(&source));
    }
    source["asrModel"] = json!(input.model_id);
    source["asrModelVersion"] = json!(input.model_version);
    source["durationMs"] = json!(input.duration_ms);
    complete(&app, &user, source, epoch, input.text.trim(), None)
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|h| h.to_str().ok())
}
fn validate_client(client: &str) -> Result<()> {
    if client.is_empty() || client.len() > 200 || client.chars().any(char::is_control) {
        return Err(Error::bad("片段ID无效"));
    }
    Ok(())
}
fn terminal(source: &Value) -> bool {
    matches!(source["status"].as_str(), Some("ready" | "no_speech"))
}
fn response(source: &Value) -> Response {
    (
        StatusCode::ACCEPTED,
        Json(json!({"id":source["id"],"status":source["status"],"audioDeleted":true})),
    )
}

fn begin(
    app: &App,
    user: &str,
    client: &str,
    checksum: &str,
    captured: &str,
    mode: &str,
) -> Result<(Value, i64)> {
    let mut conn = app.store.conn()?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let epoch: i64 = tx.query_row("SELECT epoch FROM users WHERE id=?", [user], |r| r.get(0))?;
    let existing: Option<String> = tx
        .query_row(
            "SELECT data FROM records WHERE user_id=? AND kind='source' AND client_id=?",
            params![user, client],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(raw) = existing {
        let source: Value = serde_json::from_str(&raw)?;
        if source["_hash"] != checksum {
            return Err(Error::new(
                StatusCode::CONFLICT,
                "idempotency_conflict",
                "相同片段ID已用于不同内容",
            ));
        }
        // In-flight legacy workers may still own a paid request; don't race them with a new one.
        let running: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM jobs WHERE user_id=? AND kind='transcribe' AND status='running' AND json_extract(payload,'$.sourceId')=?)", params![user,source["id"].as_str()], |r| r.get(0))?;
        if running {
            return Err(Error::new(
                StatusCode::CONFLICT,
                "audio_processing",
                "该片段仍在转写，请稍后重试",
            ));
        }
        tx.execute("UPDATE jobs SET status='cancelled',message='由设备重新提交音频' WHERE user_id=? AND kind='transcribe' AND status='queued' AND json_extract(payload,'$.sourceId')=?",params![user,source["id"].as_str()])?;
        tx.commit()?;
        return Ok((source, epoch));
    }
    let key = hash(format!("{user}:{client}").as_bytes());
    let source = json!({"id":key,"title":"语音记录","kind":"audio","createdAt":now(),"occurredAt":captured,"status":"awaiting_audio","asrMode":mode,"speakerMode":"unknown","audioDeleted":true,"_hash":checksum});
    tx.execute(
        "INSERT INTO records(user_id,kind,id,data,client_id,created) VALUES(?,'source',?,?,?,?)",
        params![
            user,
            key,
            source.to_string(),
            client,
            source["createdAt"].as_str()
        ],
    )?;
    tx.commit()?;
    Ok((source, epoch))
}

fn complete(
    app: &App,
    user: &str,
    mut source: Value,
    epoch: i64,
    text: &str,
    receipt: Option<&str>,
) -> Result<Response> {
    let empty = text.trim().is_empty();
    source["text"] = json!(text.trim());
    source["status"] = json!(if empty { "no_speech" } else { "ready" });
    source["analysisStatus"] = json!(if empty { "skipped" } else { "pending" });
    source["audioDeleted"] = json!(true);
    source
        .as_object_mut()
        .ok_or_else(Error::internal)?
        .remove("_audioPath");
    source.as_object_mut().unwrap().remove("error");
    let key = source["id"].as_str().ok_or_else(Error::internal)?;
    let mut conn = app.store.conn()?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let changed = tx.execute("UPDATE records SET data=? WHERE user_id=? AND kind='source' AND id=? AND EXISTS(SELECT 1 FROM users WHERE id=? AND epoch=?)", params![source.to_string(),user,key,user,epoch])?;
    if changed == 0 {
        return Err(Error::missing());
    }
    if !empty {
        tx.execute("INSERT OR IGNORE INTO jobs(id,user_id,kind,payload,status,created,dedup) VALUES(?,?,'formation',?,'queued',?,?)",params![id(),user,json!({"sourceId":key}).to_string(),now(),format!("formation:{key}")])?;
    }
    if let Some(receipt) = receipt {
        tx.execute(
            "UPDATE receipts SET response=NULL WHERE user_id=? AND key=?",
            params![user, receipt],
        )?;
    }
    tx.commit()?;
    Ok(response(&source))
}

fn mark_awaiting(app: &App, user: &str, source: &str, epoch: i64, error: &Error) -> Result<()> {
    app.store.conn()?.execute("UPDATE records SET data=json_set(data,'$.status','awaiting_audio','$.audioDeleted',json('true'),'$.error',?) WHERE user_id=? AND kind='source' AND id=? AND EXISTS(SELECT 1 FROM users WHERE id=? AND epoch=?)",params![error.message,user,source,user,epoch])?;
    Ok(())
}

fn legacy_result(app: &App, user: &str, source: &str) -> Result<Option<String>> {
    let conn = app.store.conn()?;
    let mut statement = conn.prepare("SELECT receipts.status,receipts.response FROM receipts JOIN jobs ON receipts.job_id=jobs.id AND receipts.user_id=jobs.user_id WHERE jobs.user_id=? AND json_extract(jobs.payload,'$.sourceId')=? ORDER BY receipts.created DESC")?;
    let rows = statement.query_map(params![user, source], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
    })?;
    for row in rows {
        let (status, text) = row?;
        if status == "completed" && text.is_some() {
            return Ok(text);
        }
        return Err(Error::new(
            StatusCode::CONFLICT,
            "outcome_unknown",
            "旧转写可能已计费；本机原音可在明确确认后重新提交",
        ));
    }
    Ok(None)
}
