//! SDK agent orchestration. Rust owns authority, durable inputs, billing and all I/O.
use crate::{
    db::Job,
    discovery::Material,
    error::{Error, Result},
    App,
};
use serde_json::{json, Value};
#[path = "agent_aihot_definition.rs"]
mod aihot_definition;
#[path = "agent_candidates.rs"]
pub(crate) mod candidates;
#[path = "agent_papers.rs"]
mod papers;
#[path = "agent_process.rs"]
mod process;
#[path = "agent_search.rs"]
mod search;
#[path = "agent_state.rs"]
mod state;
#[path = "agent_subscription_tools.rs"]
mod subscription_tools;
#[path = "agent_tools.rs"]
mod tools;

#[derive(Clone)]
pub enum Scope {
    Private {
        user: String,
        job: Option<Job>,
        epoch: i64,
        run_id: String,
        references: Vec<(String, String)>,
    },
    Public {
        edition_key: String,
        catalog_id: String,
    },
}
impl Scope {
    pub(crate) fn lane(&self) -> crate::concurrency::Lane {
        use crate::concurrency::Lane;
        match self {
            Self::Public { .. } => Lane::Digest,
            Self::Private { job: Some(job), .. } => Lane::for_job(&job.kind),
            Self::Private { job: None, .. } => Lane::Chat,
        }
    }
    pub fn private(app: &App, user: &str, job: Option<&Job>, run_id: &str) -> Result<Self> {
        Self::private_at(app, user, job, run_id, app.store.epoch(user)?)
    }
    /// Bind prepared private input to the epoch captured before reading its context.
    pub fn private_at(
        app: &App,
        user: &str,
        job: Option<&Job>,
        run_id: &str,
        expected_epoch: i64,
    ) -> Result<Self> {
        if run_id.is_empty() || run_id.len() > 200 || job.is_some_and(|j| j.user != user) {
            return Err(Error::bad("代理任务标识无效"));
        }
        let scope = Self::Private {
            user: user.into(),
            job: job.cloned(),
            epoch: expected_epoch,
            run_id: run_id.into(),
            references: vec![],
        };
        scope.check(app)?;
        Ok(scope)
    }
    pub fn public(edition_key: &str, catalog_id: &str) -> Result<Self> {
        if edition_key.is_empty()
            || edition_key.len() > 200
            || !crate::shared_public::catalog()
                .iter()
                .any(|t| t.id == catalog_id)
        {
            return Err(Error::bad("公共代理仅接受固定目录任务"));
        }
        Ok(Self::Public {
            edition_key: edition_key.into(),
            catalog_id: catalog_id.into(),
        })
    }
    pub fn with_reference(mut self, kind: &str, id: &str) -> Self {
        if let Self::Private { references, .. } = &mut self {
            references.push((kind.into(), id.into()));
        }
        self
    }
    pub(crate) fn key(&self, purpose: &str) -> String {
        let id = match self {
            Self::Private { run_id, .. } => run_id,
            Self::Public { edition_key, .. } => edition_key,
        };
        crate::hash(format!("sdk-agent-v1:{id}:{purpose}").as_bytes())
    }
    pub(crate) fn check(&self, app: &App) -> Result<()> {
        if let Self::Private {
            user,
            job,
            epoch,
            references,
            ..
        } = self
        {
            if app.store.epoch(user)? != *epoch {
                return Err(Error::new(
                    axum::http::StatusCode::CONFLICT,
                    "context_deleted",
                    "个人资料已删除，请重新开始",
                ));
            }
            let conn = app.store.conn()?;
            if let Some(j) = job {
                let active = crate::research::job_active(&conn, j)?;
                if !active {
                    return Err(Error::missing());
                }
            }
            for (kind, id) in references {
                let exists: bool = conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM records WHERE user_id=? AND kind=? AND id=?)",
                    rusqlite::params![user, kind, id],
                    |r| r.get(0),
                )?;
                if !exists {
                    return Err(Error::missing());
                }
            }
        }
        Ok(())
    }
}

pub struct Spec<'a> {
    pub mode: &'a str,
    pub purpose: &'a str,
    pub instructions: &'a str,
    pub prompt: Value,
    pub max_steps: usize,
    pub max_output_tokens: usize,
    pub tools: bool,
}
#[derive(Default)]
pub struct ToolContext {
    pub materials: Vec<Material>,
    pub profile: Value,
}
pub struct Output {
    pub text: String,
    pub materials: Vec<Material>,
    pub steps: usize,
}

pub async fn run(app: &App, scope: &Scope, spec: Spec<'_>, context: ToolContext) -> Result<Output> {
    scope.check(app)?;
    if !matches!(
        spec.mode,
        "digest" | "research" | "discussion" | "translation" | "review"
    ) || !(1..=16).contains(&spec.max_steps)
        || !(1..=8192).contains(&spec.max_output_tokens)
        || spec.purpose.is_empty()
        || spec.purpose.len() > 100
    {
        return Err(Error::bad("代理运行参数无效"));
    }
    let config = match scope {
        Scope::Private { user, .. } => crate::models::config(app, user, "text")?,
        Scope::Public { .. } => app.hosted.get("text").cloned().ok_or_else(|| {
            Error::upstream("hosted_model_unconfigured", "公共编辑尚未配置平台文本模型")
        })?,
    };
    // Validate the protocol before starting a child or recording a billable operation.
    crate::models::agent::validate_config(&config)?;
    let paper_tools = spec.tools && crate::alphaxiv::available(app, scope);
    let instructions = if paper_tools {
        format!("{}\n若问题需要论文证据，可用 papers discover 检索、papers read 读取原始论文；papers read 返回的原文满足阅读来源要求，无需重复调用网页读取。搜索摘要不能支撑事实。仅在请求需要个人收藏时使用 library，不要无关地列出私人文献库。", spec.instructions)
    } else {
        spec.instructions.to_owned()
    };
    let instructions = if spec.tools {
        format!("{}\n候选资料按页提供，不代表资料总量上限。先结合用户关注领域、目标、偏好比较多个方向，再选择原文。用list_candidates继续翻页或按关键词筛选完整候选池；hasMore为true时不能声称已看完全部资料。search、search_subscriptions或papers返回的candidateOnly条目是线索，必须read_source或papers read读取后才可引用。需要更多覆盖时细化检索；在当前时间预算内仍未读到的材料，明确是覆盖不足，不编造缺失结论。", instructions)
    } else {
        instructions
    };
    let mut prompt = spec.prompt;
    if spec.tools {
        if let Scope::Private { user, .. } = scope {
            prompt["readerPreferences"] = json!(crate::cognition::formation_context(app, user)?);
            prompt["followedInterests"] = context.profile["interests"].clone();
        }
    }
    let input = json!({"type":"start","runId":scope.key(spec.purpose),"mode":spec.mode,
        "model":{"protocol":config["protocol"].as_str().unwrap_or("openai"),"model":config["model"]},
        "prompt":prompt.to_string(),"instructions":instructions,
        "tools":if spec.tools {tools::definitions(scope, paper_tools)} else {json!([])},"maxSteps":spec.max_steps,"maxOutputTokens":spec.max_output_tokens});
    let owner = match scope {
        Scope::Private { user, .. } => Some(user.as_str()),
        Scope::Public { .. } => None,
    };
    let _child = app.concurrency.agent(scope.lane(), owner).await?;
    scope.check(app)?;
    let mut saved = state::load_or_freeze(app, scope, spec.purpose, input, context)?;
    let provider_fingerprint = crate::hash(
        format!(
            "pi-1.1-web-v3:{}:{}:{}",
            config["baseUrl"], config["protocol"], config["model"]
        )
        .as_bytes(),
    );
    if saved.context["providerFingerprint"]
        .as_str()
        .is_some_and(|old| old != provider_fingerprint)
    {
        return Err(Error::upstream(
            "idempotency_conflict",
            "研究服务已更新，请重新发起这次研究",
        ));
    }
    saved.context["providerFingerprint"] = json!(provider_fingerprint);
    state::persist(app, scope, spec.purpose, &saved)?;
    if let Some(text) = saved.complete.as_ref() {
        return Ok(Output {
            text: text.clone(),
            materials: saved.materials(),
            steps: saved.step,
        });
    }
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(720),
        process::execute(app, scope, spec.purpose, &config, &mut saved),
    )
    .await
    .map_err(|_| Error::upstream("timeout", "这次研究耗时较长，请稍后重新发起"))??;
    Ok(Output {
        text: result,
        materials: saved.materials(),
        steps: saved.step,
    })
}
/// Reusable bounded SDK path for translation and independent reviews.
pub async fn text(
    app: &App,
    job: &Job,
    expected_epoch: i64,
    purpose: &str,
    instructions: &str,
    prompt: Value,
) -> Result<String> {
    let mut scope = Scope::private_at(app, &job.user, Some(job), &job.id, expected_epoch)?;
    if let Some(id) = job.payload["reportId"].as_str() {
        scope = scope.with_reference("report", id);
    }
    if let Some(id) = job.payload["thoughtId"].as_str() {
        scope = scope.with_reference("thought", id);
    }
    let mode = if purpose.contains("translation") {
        "translation"
    } else {
        "review"
    };
    Ok(run(
        app,
        &scope,
        Spec {
            mode,
            purpose,
            instructions,
            prompt,
            max_steps: 1,
            max_output_tokens: 8192,
            tools: false,
        },
        ToolContext::default(),
    )
    .await?
    .text)
}

/// Raw prompts and provider continuation metadata are private and expire after 24 hours.
pub fn retention(app: &App) -> Result<()> {
    candidates::retention(app, chrono::Utc::now().timestamp())?;
    let cutoff = (chrono::Utc::now() - chrono::Duration::hours(24))
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let conn = app.store.conn()?;
    conn.execute(
        "DELETE FROM records WHERE kind='agent_run' AND created<?",
        [cutoff],
    )?;
    conn.execute_batch("CREATE TABLE IF NOT EXISTS public_agent_runs(run_key TEXT PRIMARY KEY,data TEXT NOT NULL,created INTEGER NOT NULL);")?;
    conn.execute(
        "DELETE FROM public_agent_runs WHERE created<?",
        [chrono::Utc::now().timestamp() - 86400],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn final_checkpoint_recovers_at_step_limit_without_another_charge() {
        let dir = std::env::temp_dir().join(format!("memhot-final-checkpoint-{}", crate::id()));
        let app = App::open(dir.clone()).unwrap();
        let (user, _) = crate::auth::create_pair(&app, "test", None).unwrap();
        let mut profile = app.store.profile(&user).unwrap();
        profile["models"]["text"] = json!({"protocol":"openai","baseUrl":"https://example.com/v1","apiKey":"private-secret","model":"test"});
        app.store.save_profile(&user, &profile).unwrap();
        let scope = Scope::private(&app, &user, None, "request").unwrap();
        let input = json!({"type":"start","runId":scope.key("review"),"mode":"review","model":{"protocol":"openai","model":"test"},"prompt":"frozen input","instructions":"rules","tools":[],"maxSteps":1,"maxOutputTokens":100});
        let mut saved =
            state::load_or_freeze(&app, &scope, "review", input, ToolContext::default()).unwrap();
        saved.step = 1;
        saved.complete = Some("validated final answer".into());
        state::persist(&app, &scope, "review", &saved).unwrap();
        let output = run(
            &app,
            &scope,
            Spec {
                mode: "review",
                purpose: "review",
                instructions: "rules",
                prompt: json!("new input is ignored on retry"),
                max_steps: 1,
                max_output_tokens: 100,
                tools: false,
            },
            ToolContext::default(),
        )
        .await
        .unwrap();
        assert_eq!(output.text, "validated final answer");
        assert_eq!(output.steps, 1);
        let calls: i64 = app
            .store
            .conn()
            .unwrap()
            .query_row(
                "SELECT count(*) FROM receipts WHERE user_id=?",
                [&user],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(calls, 0);
        drop(app);
        let _ = std::fs::remove_dir_all(dir);
    }
}
