use crate::{
    error::{Error, Result},
    id, now,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::{
    path::Path,
    sync::{Mutex, MutexGuard},
    time::Duration,
};

pub struct Store {
    connection: Mutex<Connection>,
}
#[derive(Clone, Debug)]
pub struct Job {
    pub id: String,
    pub user: String,
    pub kind: String,
    pub payload: Value,
    pub attempt: i64,
}
impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        let connection = Connection::open(path)?;
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON; PRAGMA synchronous=FULL; PRAGMA secure_delete=ON; PRAGMA cache_size=-8192;
CREATE TABLE IF NOT EXISTS users(id TEXT PRIMARY KEY,name TEXT NOT NULL,profile TEXT NOT NULL,epoch INTEGER NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS devices(token_hash TEXT PRIMARY KEY,user_id TEXT NOT NULL REFERENCES users(id),name TEXT NOT NULL,created TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS pairs(code_hash TEXT PRIMARY KEY,user_id TEXT NOT NULL REFERENCES users(id),expires INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS records(user_id TEXT NOT NULL REFERENCES users(id),kind TEXT NOT NULL,id TEXT NOT NULL,data TEXT NOT NULL,client_id TEXT,created TEXT NOT NULL,PRIMARY KEY(user_id,kind,id));
CREATE UNIQUE INDEX IF NOT EXISTS record_dedup ON records(user_id,kind,client_id) WHERE client_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS record_list ON records(user_id,kind,created DESC);
CREATE TABLE IF NOT EXISTS jobs(id TEXT PRIMARY KEY,user_id TEXT NOT NULL REFERENCES users(id),kind TEXT NOT NULL,payload TEXT NOT NULL,status TEXT NOT NULL,attempt INTEGER NOT NULL DEFAULT 0,lease_until INTEGER NOT NULL DEFAULT 0,next_at INTEGER NOT NULL DEFAULT 0,message TEXT,created TEXT NOT NULL,dedup TEXT NOT NULL,UNIQUE(user_id,dedup));
CREATE INDEX IF NOT EXISTS jobs_due ON jobs(status,next_at,lease_until);
CREATE TABLE IF NOT EXISTS scheduler_turns(user_id TEXT PRIMARY KEY REFERENCES users(id),last_turn INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS receipts(user_id TEXT NOT NULL,job_id TEXT NOT NULL,key TEXT NOT NULL,status TEXT NOT NULL,response TEXT,created INTEGER NOT NULL,PRIMARY KEY(user_id,key));
PRAGMA user_version=1;")?;
        let has_epoch: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('users') WHERE name='epoch')",
            [],
            |r| r.get(0),
        )?;
        if !has_epoch {
            connection
                .execute_batch("ALTER TABLE users ADD COLUMN epoch INTEGER NOT NULL DEFAULT 0;")?;
        }
        crate::thought_days::migrate(&connection)?;
        crate::job_scheduler::migrate(&connection)?;
        crate::accounts::migrate(&connection)?;
        crate::shared_public::migrate(&connection)?;
        crate::subscription_store::migrate(&connection)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
                .map_err(|_| Error::internal())?;
        }
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }
    pub fn conn(&self) -> Result<MutexGuard<'_, Connection>> {
        self.connection.lock().map_err(|_| Error::internal())
    }
    pub fn get(&self, user: &str, kind: &str, id: &str) -> Result<Value> {
        let raw: Option<String> = self
            .conn()?
            .query_row(
                "SELECT data FROM records WHERE user_id=? AND kind=? AND id=?",
                params![user, kind, id],
                |r| r.get(0),
            )
            .optional()?;
        serde_json::from_str(&raw.ok_or_else(Error::missing)?).map_err(Into::into)
    }
    pub fn list(&self, user: &str, kind: &str) -> Result<Vec<Value>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT data FROM records WHERE user_id=? AND kind=? ORDER BY created DESC LIMIT 60",
        )?;
        let rows = stmt.query_map(params![user, kind], |r| r.get::<_, String>(0))?;
        rows.map(|r| Ok(serde_json::from_str(&r?)?)).collect()
    }
    /// Internal processing must not inherit the home screen's sixty-item page.
    pub fn list_all(&self, user: &str, kind: &str) -> Result<Vec<Value>> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare("SELECT data FROM records WHERE user_id=? AND kind=? ORDER BY created,id")?;
        let rows = stmt.query_map(params![user, kind], |r| r.get::<_, String>(0))?;
        rows.map(|r| Ok(serde_json::from_str(&r?)?)).collect()
    }
    pub fn put(
        &self,
        user: &str,
        kind: &str,
        value: &Value,
        client: Option<&str>,
    ) -> Result<Value> {
        let key = value["id"].as_str().ok_or_else(Error::internal)?;
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        tx.execute("INSERT INTO records(user_id,kind,id,data,client_id,created) VALUES(?,?,?,?,?,?) ON CONFLICT(user_id,kind,id) DO UPDATE SET data=excluded.data",params![user,kind,key,value.to_string(),client,value["createdAt"].as_str().unwrap_or("")])?;
        if kind == "thought" {
            crate::thought_days::initialize(&tx, user, value)?;
        }
        tx.commit()?;
        Ok(value.clone())
    }
    pub fn by_client(&self, user: &str, kind: &str, client: &str) -> Result<Option<Value>> {
        let raw: Option<String> = self
            .conn()?
            .query_row(
                "SELECT data FROM records WHERE user_id=? AND kind=? AND client_id=?",
                params![user, kind, client],
                |r| r.get(0),
            )
            .optional()?;
        raw.map(|s| serde_json::from_str(&s).map_err(Into::into))
            .transpose()
    }
    pub fn remove(&self, user: &str, kind: &str, key: &str) -> Result<()> {
        if self.conn()?.execute(
            "DELETE FROM records WHERE user_id=? AND kind=? AND id=?",
            params![user, kind, key],
        )? == 0
        {
            return Err(Error::missing());
        }
        Ok(())
    }
    pub fn epoch(&self, user: &str) -> Result<i64> {
        self.conn()?
            .query_row("SELECT epoch FROM users WHERE id=?", [user], |r| r.get(0))
            .map_err(Into::into)
    }
    pub fn profile(&self, user: &str) -> Result<Value> {
        let raw: String = self
            .conn()?
            .query_row("SELECT profile FROM users WHERE id=?", [user], |r| r.get(0))
            .optional()?
            .ok_or_else(Error::auth)?;
        Ok(serde_json::from_str(&raw)?)
    }
    pub fn save_profile(&self, user: &str, p: &Value) -> Result<()> {
        self.conn()?.execute(
            "UPDATE users SET profile=?,name=? WHERE id=?",
            params![p.to_string(), p["name"].as_str().unwrap_or("我"), user],
        )?;
        Ok(())
    }
    pub fn queue(&self, user: &str, kind: &str, payload: Value, dedup: &str) -> Result<Value> {
        let conn = self.conn()?;
        let key = id();
        let created = now();
        conn.execute("INSERT OR IGNORE INTO jobs(id,user_id,kind,payload,status,created,dedup) VALUES(?,?,?,?,'queued',?,?)",params![key,user,kind,payload.to_string(),created,dedup])?;
        conn.query_row(
            "SELECT id,status FROM jobs WHERE user_id=? AND dedup=?",
            params![user, dedup],
            |r| Ok(json!({"id":r.get::<_,String>(0)?,"status":r.get::<_,String>(1)?})),
        )
        .map_err(Into::into)
    }
    pub fn jobs(&self, user: &str) -> Result<Vec<Value>> {
        let conn = self.conn()?;
        let mut stmt=conn.prepare("SELECT id,kind,status,message,created,EXISTS(SELECT 1 FROM receipts WHERE receipts.user_id=jobs.user_id AND receipts.job_id=jobs.id AND receipts.status IN ('pending','paid_incomplete')),status='failed' AND NOT EXISTS(SELECT 1 FROM jobs child WHERE child.user_id=jobs.user_id AND child.dedup='retry:'||jobs.id) FROM jobs WHERE user_id=? ORDER BY created DESC LIMIT 30")?;
        let rows=stmt.query_map([user],|r|Ok(json!({"id":r.get::<_,String>(0)?,"kind":r.get::<_,String>(1)?,"status":r.get::<_,String>(2)?,"message":r.get::<_,Option<String>>(3)?,"createdAt":r.get::<_,String>(4)?,"requiresChargeConfirmation":r.get::<_,bool>(5)?,"retryable":r.get::<_,bool>(6)?})))?;
        rows.map(|r| r.map_err(Into::into)).collect()
    }
    /// Select any durable lane, preserving tenant rotation for administrative drains.
    pub fn claim(&self) -> Result<Option<Job>> {
        crate::job_scheduler::claim(self, None)
    }
    /// Reserve a durable lane's single global slot across all worker connections.
    pub fn claim_lane(&self, lane: crate::concurrency::Lane) -> Result<Option<Job>> {
        crate::job_scheduler::claim(self, Some(lane))
    }
    pub fn finish(&self, job: &Job, records: &[(&str, Value)], message: &str) -> Result<bool> {
        let mut conn = self.conn()?;
        crate::persistence::finish(&mut conn, job, records, message)
    }
    pub fn checkpoint(&self, job: &Job, records: &[(&str, Value)]) -> Result<bool> {
        let mut conn = self.conn()?;
        crate::persistence::checkpoint(&mut conn, job, records)
    }
    pub fn fail(&self, job: &Job, error: &Error) -> Result<()> {
        let retry = job.attempt < 3
            && matches!(
                error.code,
                "network" | "timeout" | "rate_limited" | "upstream" | "busy"
            );
        self.conn()?.execute("UPDATE jobs SET status=?,message=?,lease_until=0,next_at=? WHERE id=? AND user_id=? AND status='running' AND attempt=?",params![if error.code=="quota_exhausted"{"blocked"}else if retry{"queued"}else{"failed"},error.message,chrono::Utc::now().timestamp()+60*job.attempt,job.id,job.user,job.attempt])?;
        Ok(())
    }
    pub fn users(&self) -> Result<Vec<String>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare("SELECT id FROM users ORDER BY id")?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        rows.map(|r| r.map_err(Into::into)).collect()
    }
}
