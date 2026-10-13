//! Fixed public interests and private research never share inputs or durable bodies.
use crate::{
    db::Job,
    error::{Error, Result},
    hash, now, shared_public, App,
};
use rusqlite::params;
use serde_json::{json, Value};
#[path = "digest_publication.rs"]
mod publication;
pub(crate) use publication::schedule_publication;

pub struct Interests {
    pub public: Vec<&'static shared_public::CatalogTopic>,
    pub private: Vec<String>,
    pub feeds: Vec<Value>,
}
pub fn interests(app: &App, user: &str) -> Result<Interests> {
    let profile = app.store.profile(user)?;
    let mut values: Vec<String> = profile["interests"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect();
    for item in crate::cognition::context(app, user)? {
        if item["type"] == "interest" {
            if let Some(value) = item["statement"].as_str() {
                if !values
                    .iter()
                    .any(|v| v.trim().eq_ignore_ascii_case(value.trim()))
                {
                    values.push(value.to_owned());
                }
            }
        }
    }
    let public = shared_public::match_topics(&values);
    let private = values
        .into_iter()
        .filter(|v| shared_public::match_topics(&[v.clone()]).is_empty())
        .collect();
    Ok(Interests {
        public,
        private,
        feeds: profile["feeds"].as_array().cloned().unwrap_or_default(),
    })
}
/// Enqueue one bounded public edition task per catalog topic; private input stays in a separate task.
fn dispatch(app: &App, job: &Job, syndicated: bool) -> Result<Vec<(&'static str, Value)>> {
    let interests = interests(app, &job.user)?;
    let mut payloads: Vec<Value> = interests
        .public
        .iter()
        .map(|topic| json!({"kind":"digest","digestStage":"public","catalogTopicId":topic.id}))
        .collect();
    if !interests.private.is_empty() || !interests.feeds.is_empty() {
        payloads.push(json!({"kind":"digest","digestStage":"private"}));
    }
    let mut conn = app.store.conn()?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let active: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM jobs WHERE id=? AND user_id=? AND status='running' AND attempt=?)",params![job.id,job.user,job.attempt],|r|r.get(0))?;
    if !active {
        return Err(Error::missing());
    }
    let immediate: bool = tx.query_row("SELECT COALESCE(json_extract(payload,'$.deliverImmediately'),0) FROM jobs WHERE id=? AND user_id=?",params![job.id,job.user],|r|r.get(0))?;
    for mut payload in payloads {
        // Read the live manual-refresh flag before excluding night AI work:
        // the claimed Job can predate a request to deliver immediately.
        if syndicated
            && !immediate
            && payload["catalogTopicId"] == "ai"
            && job.payload["availableAt"].is_string()
        {
            continue;
        }
        for field in ["availableAt", "researchDate", "localDate", "date"] {
            if let Some(value) = job.payload.get(field) {
                payload[field] = value.clone();
            }
        }
        if immediate {
            payload.as_object_mut().map(|p| p.remove("availableAt"));
            payload["deliverImmediately"] = json!(true);
        }
        let child = format!(
            "digest-child:{}:{}:{}",
            job.id, payload["digestStage"], payload["catalogTopicId"]
        );
        tx.execute("INSERT OR IGNORE INTO jobs(id,user_id,kind,payload,status,created,dedup) VALUES(?,?,'digest',?,'queued',?,?)",params![crate::id(),job.user,payload.to_string(),now(),child])?;
    }
    tx.commit()?;
    Ok(vec![])
}
pub async fn run(app: &App, job: &Job) -> Result<Vec<(&'static str, Value)>> {
    if job.payload["digestStage"] != "public" {
        return dispatch(app, job, crate::aihot::enabled());
    }
    crate::billing::ensure_subscription(app, &job.user)?;
    let topic_id = job.payload["catalogTopicId"]
        .as_str()
        .ok_or_else(|| Error::bad("公共日报领域无效"))?;
    if !interests(app, &job.user)?
        .public
        .iter()
        .any(|t| t.id == topic_id)
    {
        return Ok(vec![]);
    }
    let result = shared_public::edition(app, topic_id, "zh-CN").await?;
    let edition = result.edition.ok_or_else(|| {
        Error::upstream(
            if result.cache_status == "refreshing" {
                "busy"
            } else {
                "public_digest_unavailable"
            },
            result
                .warning
                .as_deref()
                .unwrap_or("公共日报暂不可用，尚未取得的内容不会被当作新材料"),
        )
    })?;
    let edition_id = edition["id"].as_str().ok_or_else(Error::internal)?;
    let key = format!(
        "public-ref-{}",
        hash(format!("{}:{edition_id}", job.user).as_bytes())
    );
    let dismissed: bool = app.store.conn()?.query_row(
        "SELECT EXISTS(SELECT 1 FROM records WHERE user_id=? AND kind='report_tombstone' AND id=?)",
        params![job.user, key],
        |r| r.get(0),
    )?;
    if dismissed {
        return Ok(vec![]);
    }
    let old = app.store.get(&job.user, "report", &key).ok();
    let mut report = json!({"id":key,"kind":"digest","publicEditionId":edition_id,"catalogTopicId":topic_id,"createdAt":edition["createdAt"].as_str().map(str::to_owned).unwrap_or_else(now),"publicCacheStatus":result.cache_status,"publicWarning":result.warning});
    for field in ["researchDate", "localDate"] {
        if let Some(value) = job.payload.get(field) {
            report[field] = value.clone();
        }
    }
    // An explicit refresh can reveal the same edition immediately; scheduled work never re-hides it.
    if let Some(next) = job.payload["availableAt"].as_str() {
        let available = match old.as_ref() {
            Some(old) => old["availableAt"]
                .as_str()
                .map(|prior| if prior < next { prior } else { next }),
            None => Some(next),
        };
        if let Some(available) = available {
            report["availableAt"] = json!(available);
        }
    }
    Ok(vec![("report", report)])
}

#[cfg(test)]
mod dispatch_tests {
    use super::*;
    #[test]
    fn manual_refresh_after_claim_still_dispatches_ai() {
        let dir = std::env::temp_dir().join(format!("memhot-digest-dispatch-{}", crate::id()));
        let app = App::open(dir.clone()).unwrap();
        let (user, _) = crate::auth::create_pair(&app, "test", None).unwrap();
        let mut p = app.store.profile(&user).unwrap();
        p["interests"] = json!(["AI"]);
        app.store.save_profile(&user, &p).unwrap();
        for immediate in [false, true] {
            app.store
                .queue(
                    &user,
                    "digest",
                    json!({"kind":"digest","availableAt":"2099-01-01T00:00:00Z"}),
                    &format!("night-{immediate}"),
                )
                .unwrap();
            let job = app.store.claim().unwrap().unwrap();
            if immediate {
                app.store.conn().unwrap().execute("UPDATE jobs SET payload=json_set(json_remove(payload,'$.availableAt'),'$.deliverImmediately',json('true')) WHERE id=?",[&job.id]).unwrap();
            }
            dispatch(&app, &job, true).unwrap();
            assert!(app.store.finish(&job, &[], "done").unwrap());
            let count:i64=app.store.conn().unwrap().query_row("SELECT count(*) FROM jobs WHERE json_extract(payload,'$.digestStage')='public' AND json_extract(payload,'$.catalogTopicId')='ai'",[],|r|r.get(0)).unwrap();
            assert_eq!(count, if immediate { 1 } else { 0 });
        }
        drop(app);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
