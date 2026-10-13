//! Frozen start frames and checkpoint/tool replay, separated by immutable audience.
use super::{Scope, ToolContext};
use crate::{
    discovery::Material,
    error::{Error, Result},
    App,
};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Serialize, Deserialize)]
pub(super) struct Saved {
    pub input: Value,
    pub context: Value,
    pub step: usize,
    pub messages_json: String,
    pub pending_models: Vec<Value>,
    pub pending_tools: Vec<Value>,
    pub complete: Option<String>,
    pub epoch: Option<i64>,
    pub created_at: String,
}
impl Saved {
    pub fn materials(&self) -> Vec<Material> {
        self.context["materials"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(decode)
            .collect()
    }
    pub fn set_materials(&mut self, materials: &[Material]) {
        self.context["materials"] =
            json!(materials.iter().take(40).map(encode).collect::<Vec<_>>());
    }
}
pub(super) fn encode(m: &Material) -> Value {
    json!({"title":m.title,"url":m.url,"text":m.text,"published":m.published,"discovered":m.discovered,"discoveredVia":m.discovered_via,"publicationSource":m.publication_source,"publisher":m.publisher,"fulltext":m.fulltext,"extraction":m.extraction()})
}
fn decode(v: &Value) -> Option<Material> {
    Some(Material {
        title: v["title"].as_str()?.into(),
        url: v["url"].as_str()?.into(),
        text: v["text"].as_str()?.into(),
        published: v["published"].as_str().map(str::to_owned),
        discovered: v["discovered"].as_str().map(str::to_owned),
        discovered_via: v["discoveredVia"].as_str().map(str::to_owned),
        publication_source: match v["publicationSource"].as_str().unwrap_or("") {
            "page" => "page",
            "crossref_publication" => "crossref_publication",
            "aggregator_submission" => "aggregator_submission",
            "search_reported_publication" => "search_reported_publication",
            "feed" => "feed",
            "feed_publication" => "feed_publication",
            "feed_published" => "feed_published",
            "feed_updated" => "feed_updated",
            "page_metadata" => "page_metadata",
            "original_page_metadata" => "original_page_metadata",
            "arxiv_publication" => "arxiv_publication",
            _ => "unknown",
        },
        publisher: v["publisher"].as_str().unwrap_or("").into(),
        fulltext: v["fulltext"] == true,
        pdf: if v["extraction"].is_null() {
            None
        } else {
            Some(crate::discovery::PdfExtraction::from_value(
                &v["extraction"],
            )?)
        },
    })
}
fn public_schema(app: &App) -> Result<()> {
    app.store.conn()?.execute_batch("CREATE TABLE IF NOT EXISTS public_agent_runs(run_key TEXT PRIMARY KEY,data TEXT NOT NULL,created INTEGER NOT NULL);")?;
    Ok(())
}
fn raw(app: &App, scope: &Scope, key: &str) -> Result<Option<String>> {
    match scope {
        Scope::Private { user, .. } => app
            .store
            .conn()?
            .query_row(
                "SELECT data FROM records WHERE user_id=? AND kind='agent_run' AND id=?",
                rusqlite::params![user, key],
                |r| r.get(0),
            )
            .optional()
            .map_err(Into::into),
        Scope::Public { .. } => {
            public_schema(app)?;
            app.store
                .conn()?
                .query_row(
                    "SELECT data FROM public_agent_runs WHERE run_key=?",
                    [key],
                    |r| r.get(0),
                )
                .optional()
                .map_err(Into::into)
        }
    }
}
pub(super) fn load_or_freeze(
    app: &App,
    scope: &Scope,
    purpose: &str,
    input: Value,
    context: ToolContext,
) -> Result<Saved> {
    scope.check(app)?;
    let key = scope.key(purpose);
    if let Some(raw) = raw(app, scope, &key)? {
        if raw.len() > 8 * 1024 * 1024 {
            return Err(Error::upstream(
                "agent_state_invalid",
                "代理检查点超出大小限制",
            ));
        }
        let saved: Saved = serde_json::from_str(&raw)?;
        if let Scope::Private { epoch, .. } = scope {
            if saved.epoch != Some(*epoch) {
                return Err(Error::upstream(
                    "context_deleted",
                    "代理任务的个人上下文已删除",
                ));
            }
        }
        // A retry always reuses the original input. Model identity is immutable per run.
        if saved.input["model"] != input["model"] || saved.input["mode"] != input["mode"] {
            return Err(Error::upstream(
                "idempotency_conflict",
                "代理任务的模型或模式已变化，请创建新任务",
            ));
        }
        return Ok(saved);
    }
    let mut profile = json!({});
    if matches!(scope, Scope::Private { .. }) {
        for k in ["feeds", "interests", "_discoveryKind", "_seenDigestUrls"] {
            if let Some(v) = context.profile.get(k) {
                profile[k] = v.clone();
            }
        }
    }
    let refs = match scope {
        Scope::Private { references, .. } => json!(references),
        _ => json!([]),
    };
    let mut saved = Saved {
        input,
        context: json!({"materials":[],"profile":profile,"references":refs}),
        step: 0,
        messages_json: String::new(),
        pending_models: vec![],
        pending_tools: vec![],
        complete: None,
        epoch: match scope {
            Scope::Private { epoch, .. } => Some(*epoch),
            _ => None,
        },
        created_at: crate::now(),
    };
    saved.set_materials(&context.materials);
    persist(app, scope, purpose, &saved)?;
    Ok(saved)
}
pub(super) fn persist(app: &App, scope: &Scope, purpose: &str, saved: &Saved) -> Result<()> {
    scope.check(app)?;
    let data = serde_json::to_string(saved)?;
    if data.len() > 8 * 1024 * 1024 {
        return Err(Error::upstream(
            "agent_state_invalid",
            "代理检查点超出大小限制",
        ));
    }
    let key = scope.key(purpose);
    match scope {
        Scope::Private {
            user,
            epoch,
            job,
            references,
            ..
        } => {
            let mut conn = app.store.conn()?;
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let current: i64 =
                tx.query_row("SELECT epoch FROM users WHERE id=?", [user], |r| r.get(0))?;
            if current != *epoch {
                return Err(Error::upstream(
                    "context_deleted",
                    "代理任务的个人上下文已删除",
                ));
            }
            if let Some(j) = job {
                let active:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM jobs WHERE id=? AND user_id=? AND status='running' AND attempt=? AND lease_until>=?)",rusqlite::params![j.id,user,j.attempt,chrono::Utc::now().timestamp()],|r|r.get(0))?;
                if !active {
                    return Err(Error::missing());
                }
            }
            for (kind, id) in references {
                let exists: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM records WHERE user_id=? AND kind=? AND id=?)",
                    rusqlite::params![user, kind, id],
                    |r| r.get(0),
                )?;
                if !exists {
                    return Err(Error::missing());
                }
            }
            tx.execute("INSERT INTO records(user_id,kind,id,data,created) VALUES(?,'agent_run',?,?,?) ON CONFLICT(user_id,kind,id) DO UPDATE SET data=excluded.data",rusqlite::params![user,key,data,saved.created_at])?;
            tx.commit()?;
        }
        Scope::Public { .. } => {
            public_schema(app)?;
            app.store.conn()?.execute("INSERT INTO public_agent_runs(run_key,data,created) VALUES(?,?,?) ON CONFLICT(run_key) DO UPDATE SET data=excluded.data",rusqlite::params![key,data,chrono::Utc::now().timestamp()])?;
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pdf_read_boundary_survives_checkpoint_without_becoming_quotable_text() {
        let mut value = json!({"title":"A study","url":"https://example.org/study.pdf",
            "text":"Original page text", "fulltext":true,
            "extraction":{"format":"pdf","pagesRead":2,"totalPages":9,"truncated":true}});
        let material = decode(&value).unwrap();
        let restored = decode(&encode(&material)).unwrap();
        assert_eq!(restored.pdf, material.pdf);
        assert_eq!(restored.evidence()["extraction"], value["extraction"]);
        assert_eq!(restored.text, "Original page text");
        value["extraction"]["truncated"] = json!(false);
        assert!(decode(&value).is_none());
        value["extraction"] = Value::Null;
        assert!(decode(&value).unwrap().pdf.is_none());
    }
    #[test]
    fn frozen_inputs_survive_new_discovery_and_never_store_profile_credentials() {
        let dir = std::env::temp_dir().join(format!("memhot-agent-state-{}", crate::id()));
        let app = App::open(dir.clone()).unwrap();
        let (user, _) = crate::auth::create_pair(&app, "test", None).unwrap();
        let scope = Scope::private(&app, &user, None, "req").unwrap();
        let a = json!({"mode":"research","model":{"model":"test"},"prompt":"original"});
        let state = load_or_freeze(
            &app,
            &scope,
            "test",
            a.clone(),
            ToolContext {
                profile: json!({"search":{"apiKey":"secret"},"interests":["science"]}),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(!state.context.to_string().contains("secret"));
        let mut b = a;
        b["prompt"] = json!("different");
        assert_eq!(
            load_or_freeze(&app, &scope, "test", b, ToolContext::default())
                .unwrap()
                .input["prompt"],
            "original"
        );
        app.store
            .conn()
            .unwrap()
            .execute("UPDATE users SET epoch=epoch+1 WHERE id=?", [&user])
            .unwrap();
        assert!(scope.check(&app).is_err());
        drop(app);
        let _ = std::fs::remove_dir_all(dir);
    }
}
