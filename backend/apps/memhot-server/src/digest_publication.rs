//! Syndicated issues are fetched after the publisher's cutoff, independently of night research.
use crate::{
    error::{Error, Result},
    App,
};
use chrono::{DateTime, TimeZone, Timelike, Utc};
use rusqlite::params;
use serde_json::json;

pub(crate) fn schedule_publication(app: &App, user: &str, at: DateTime<Utc>) -> Result<()> {
    if crate::aihot::enabled() {
        schedule_at(app, user, at)?;
    }
    Ok(())
}
fn schedule_at(app: &App, user: &str, at: DateTime<Utc>) -> Result<()> {
    let publisher_time = at.with_timezone(&chrono_tz::Asia::Shanghai);
    let minute = publisher_time.hour() * 60 + publisher_time.minute();
    // Three bounded discovery opportunities cover upstream delay. No model calls
    // are made; completed old/unknown model operations are never restarted.
    let Some(slot) = [485, 500, 540].iter().rposition(|&m| minute >= m) else {
        return Ok(());
    };
    if !super::interests(app, user)?
        .public
        .iter()
        .any(|t| t.id == "ai")
    {
        return Ok(());
    }
    let date = publisher_time.date_naive().to_string();
    let edition = format!("aihot-daily-{date}");
    let conn = app.store.conn()?;
    let delivered: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM records WHERE user_id=? AND kind='report' AND json_extract(data,'$.publicEditionId')=?)", params![user,edition], |r| r.get(0))?;
    let pending: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM jobs WHERE user_id=? AND kind='digest' AND status IN ('queued','running') AND json_extract(payload,'$.digestStage')='public' AND json_extract(payload,'$.catalogTopicId')='ai')",[user],|r|r.get(0))?;
    drop(conn);
    if delivered || pending {
        return Ok(());
    }
    let profile = app.store.profile(user)?;
    let tz = profile["timezone"]
        .as_str()
        .unwrap_or("Asia/Shanghai")
        .parse::<chrono_tz::Tz>()
        .unwrap_or(chrono_tz::Asia::Shanghai);
    let hour = profile["digestHour"].as_u64().unwrap_or(8).min(23) as u32;
    // Align to publication time, not retry/startup time. Outside China an issue
    // can appear after today's preferred hour and belongs in the next delivery.
    let nominal = publisher_time
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_utc();
    let mut local_date = nominal.with_timezone(&tz).date_naive();
    let resolve = |date: chrono::NaiveDate| {
        tz.from_local_datetime(&date.and_hms_opt(hour, 0, 0).ok_or_else(Error::internal)?)
            .earliest()
            .or_else(|| {
                tz.from_local_datetime(&date.and_hms_opt((hour + 1).min(23), 0, 0).unwrap())
                    .earliest()
            })
            .ok_or_else(Error::internal)
            .map(|d| d.with_timezone(&Utc))
    };
    let mut requested = resolve(local_date)?;
    if requested < nominal {
        local_date = local_date.succ_opt().ok_or_else(Error::internal)?;
        requested = resolve(local_date)?;
    }
    app.store.queue(user,"digest",json!({"kind":"digest","digestStage":"public","catalogTopicId":"ai","localDate":local_date.to_string(),"availableAt":requested.max(at).to_rfc3339()}),&format!("aihot-publication:{date}:{slot}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn time(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }
    #[test]
    fn publication_waits_for_upstream_is_bounded_and_stops_after_owned_delivery() {
        let dir = std::env::temp_dir().join(format!("memhot-publication-{}", crate::id()));
        let app = App::open(dir.clone()).unwrap();
        let (user, _) = crate::auth::create_pair(&app, "test", None).unwrap();
        let mut profile = app.store.profile(&user).unwrap();
        profile["interests"] = json!(["AI"]);
        profile["digestHour"] = json!(21);
        app.store.save_profile(&user, &profile).unwrap();
        let count = || {
            app.store
                .conn()
                .unwrap()
                .query_row(
                    "SELECT count(*) FROM jobs WHERE dedup LIKE 'aihot-publication:%'",
                    [],
                    |r| r.get::<_, i64>(0),
                )
                .unwrap()
        };
        schedule_at(&app, &user, time("2026-10-13T00:04:59Z")).unwrap();
        assert_eq!(count(), 0);
        app.store
            .queue(
                &user,
                "digest",
                json!({"digestStage":"public","catalogTopicId":"ai"}),
                "old-paid-blocked",
            )
            .unwrap();
        app.store
            .conn()
            .unwrap()
            .execute(
                "UPDATE jobs SET status='blocked' WHERE dedup='old-paid-blocked'",
                [],
            )
            .unwrap();
        schedule_at(&app, &user, time("2026-10-13T00:05:00Z")).unwrap();
        assert_eq!(count(), 1);
        schedule_at(&app, &user, time("2026-10-13T01:00:00Z")).unwrap();
        assert_eq!(count(), 1); // pending job owns work
        let raw: String = app
            .store
            .conn()
            .unwrap()
            .query_row(
                "SELECT payload FROM jobs WHERE dedup LIKE 'aihot-publication:%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&raw).unwrap()["availableAt"],
            "2026-10-13T13:00:00+00:00"
        );
        app.store
            .conn()
            .unwrap()
            .execute(
                "UPDATE jobs SET status='completed' WHERE dedup LIKE 'aihot-publication:%'",
                [],
            )
            .unwrap();
        schedule_at(&app, &user, time("2026-10-13T00:10:00Z")).unwrap();
        assert_eq!(count(), 1); // same slot cannot replay
        schedule_at(&app, &user, time("2026-10-13T00:20:00Z")).unwrap();
        assert_eq!(count(), 2);
        app.store
            .conn()
            .unwrap()
            .execute(
                "UPDATE jobs SET status='completed' WHERE dedup LIKE 'aihot-publication:%'",
                [],
            )
            .unwrap();
        app.store.put(&user,"report",&json!({"id":"ref","publicEditionId":"aihot-daily-2026-10-13","createdAt":crate::now()}),None).unwrap();
        schedule_at(&app, &user, time("2026-10-13T01:00:00Z")).unwrap();
        assert_eq!(count(), 2);
        let (other, _) = crate::auth::create_pair(&app, "other", None).unwrap();
        schedule_at(&app, &other, time("2026-10-13T01:00:00Z")).unwrap();
        assert_eq!(count(), 2);
        let mut p = app.store.profile(&other).unwrap();
        p["interests"] = json!(["AI"]);
        p["timezone"] = json!("America/Los_Angeles");
        p["digestHour"] = json!(8);
        app.store.save_profile(&other, &p).unwrap();
        schedule_at(&app, &other, time("2026-10-13T00:05:00Z")).unwrap();
        let raw: String = app
            .store
            .conn()
            .unwrap()
            .query_row("SELECT payload FROM jobs WHERE user_id=?", [&other], |r| {
                r.get(0)
            })
            .unwrap();
        let payload: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(payload["availableAt"], "2026-10-13T15:00:00+00:00");
        assert_eq!(payload["localDate"], "2026-10-13");
        drop(app);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
