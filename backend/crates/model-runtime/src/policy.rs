//! Workload policy：按工作负载的有序候选表、预算与硬要求。
//!
//! 内置默认与现状逐字段等价（单候选、单 attempt、无替代）；用户用
//! `model_workloads.json` 显式覆盖后才激活多候选/重试。策略只引用
//! target id，不存凭据；调用开始冻结候选快照（见 execution）。

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::path::Path;
use std::time::Duration;

use provider_config::ProviderCapability;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum PolicyError {
    #[error("workload id must be non-empty")]
    EmptyWorkload,
    #[error("policy for `{0}` must list at least one candidate")]
    EmptyCandidates(String),
    #[error("policy for `{0}` must allow max_attempts >= 1")]
    InvalidMaxAttempts(String),
    #[error("invalid workload policy file: {0}")]
    InvalidFile(String),
    #[error("unsupported workload policy file version: {0}")]
    UnsupportedVersion(u64),
}

/// 一个工作负载的执行策略。字段序列化 camelCase，与
/// `HostPaths.data_dir/model_workloads.json` 一致。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkloadPolicy {
    pub workload: String,
    /// 有序候选 target id 表；第一个是主候选。
    pub candidates: Vec<String>,
    /// false：只用第一个可用候选（现状默认）。
    pub allow_fallback: bool,
    /// 跨所有候选的总 attempt 上限（含同 target 重试）。
    pub max_attempts: u32,
    /// workload 级总 deadline 兜底；实际取 min(此值, ModelCall.deadline)。
    pub deadline_ms: u64,
    /// 候选必须声明的能力（硬要求，不因 fallback 静默放宽）。
    pub requires: BTreeSet<ProviderCapability>,
}

impl WorkloadPolicy {
    fn validate(&self) -> Result<(), PolicyError> {
        if self.workload.trim().is_empty() {
            return Err(PolicyError::EmptyWorkload);
        }
        if self.candidates.is_empty() {
            return Err(PolicyError::EmptyCandidates(self.workload.clone()));
        }
        if self.max_attempts == 0 {
            return Err(PolicyError::InvalidMaxAttempts(self.workload.clone()));
        }
        Ok(())
    }

    pub fn deadline(&self) -> Duration {
        Duration::from_millis(self.deadline_ms)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PolicySet {
    workloads: BTreeMap<String, WorkloadPolicy>,
}

impl PolicySet {
    pub fn new() -> Self {
        Self::default()
    }

    /// 未知 workload 是配置错误：明确报错，不静默给默认策略。
    pub fn resolve(&self, workload: &str) -> Result<&WorkloadPolicy, PolicyError> {
        self.workloads.get(workload).ok_or_else(|| {
            PolicyError::InvalidFile(format!("workload `{workload}` has no policy registered"))
        })
    }

    pub fn upsert(&mut self, policy: WorkloadPolicy) -> Result<(), PolicyError> {
        policy.validate()?;
        self.workloads.insert(policy.workload.clone(), policy);
        Ok(())
    }

    pub fn workloads(&self) -> impl Iterator<Item = &WorkloadPolicy> {
        self.workloads.values()
    }
}

/// 已知工作负载（公开常量，调用方与测试引用，避免字符串漂移）。
pub mod workload {
    pub const ORGANIZER_STRUCTURED: &str = "organizer.structured";
    pub const MEMORY_PLAN: &str = "memory.plan";
    pub const MEMORY_SYNTHESIZE: &str = "memory.synthesize";
    pub const PROJECT_ANSWER: &str = "project.answer";
    pub const GENERAL_CHAT: &str = "general.chat";
    pub const MEMORY_JUDGE: &str = "memory.judge";
    pub const MEMORY02_FORMATION: &str = "memory02.formation";
    pub const MEMORY02_RECALL: &str = "memory02.recall";
    pub const SCENE_JUDGE: &str = "scene.judge";
    pub const MINUTES_SUMMARIZE: &str = "minutes.summarize";
    pub const TRANSCRIPT_CORRECT: &str = "transcript.correct";
}

/// 内置默认：单候选 `default_target`、无替代、单 attempt；deadline/output
/// 能力与 §3.6 兼容基线表逐项相等（迁移前行为）。
pub fn builtin_defaults(default_target: &str) -> PolicySet {
    let mut set = PolicySet::new();
    let entry = |workload: &'static str, deadline_ms: u64, requires: &[ProviderCapability]| {
        WorkloadPolicy {
            workload: workload.to_owned(),
            candidates: vec![default_target.to_owned()],
            allow_fallback: false,
            max_attempts: 1,
            deadline_ms,
            requires: requires.iter().copied().collect(),
        }
    };
    for policy in [
        entry(
            workload::ORGANIZER_STRUCTURED,
            300_000,
            &[ProviderCapability::StructuredOutput],
        ),
        entry(
            workload::MEMORY_PLAN,
            30_000,
            &[ProviderCapability::StructuredOutput],
        ),
        entry(
            workload::MEMORY_SYNTHESIZE,
            30_000,
            &[ProviderCapability::ChatCompletion],
        ),
        entry(
            workload::PROJECT_ANSWER,
            45_000,
            &[ProviderCapability::StructuredOutput],
        ),
        entry(
            workload::GENERAL_CHAT,
            60_000,
            &[ProviderCapability::ChatCompletion],
        ),
        entry(
            workload::MEMORY_JUDGE,
            20_000,
            &[ProviderCapability::ChatCompletion],
        ),
        entry(workload::MEMORY02_FORMATION, 180_000, &[ProviderCapability::StructuredOutput]),
        entry(workload::MEMORY02_RECALL, 30_000, &[ProviderCapability::StructuredOutput]),
        entry(
            workload::SCENE_JUDGE,
            90_000,
            &[ProviderCapability::StructuredOutput],
        ),
        entry(
            workload::MINUTES_SUMMARIZE,
            60_000,
            &[ProviderCapability::ChatCompletion],
        ),
        entry(
            workload::TRANSCRIPT_CORRECT,
            30_000,
            &[ProviderCapability::StructuredOutput],
        ),
    ] {
        set.upsert(policy).expect("builtin policy must validate");
    }
    set
}

/// 覆盖合并：同 workload 的覆盖内置项，未提及的保留内置。覆盖项必须完整
/// 合法（不支持在覆盖文件里只改一个字段）。
pub fn merge_overrides(builtins: PolicySet, overrides: PolicySet) -> PolicySet {
    let mut merged = builtins;
    for policy in overrides.workloads() {
        let policy = policy.clone();
        // 覆盖项合法性在 upsert 校验；内置键被完整替换。
        merged
            .upsert(policy)
            .expect("override policy must validate");
    }
    merged
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OverridesFile {
    version: u64,
    workloads: Vec<WorkloadPolicy>,
}

/// 解析 `model_workloads.json`。文件不存在 → 空 PolicySet；存在但非法 →
/// Err（调用方决定降级行为；host 侧记录错误并保留内置策略）。
pub fn load_overrides_file(path: &Path) -> Result<PolicySet, PolicyError> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(PolicySet::new());
        }
        Err(error) => {
            return Err(PolicyError::InvalidFile(format!("read {path:?}: {error}")));
        }
    };
    let parsed: OverridesFile =
        serde_json::from_str(&raw).map_err(|error| PolicyError::InvalidFile(error.to_string()))?;
    if parsed.version != 1 {
        return Err(PolicyError::UnsupportedVersion(parsed.version));
    }
    let mut set = PolicySet::new();
    for policy in parsed.workloads {
        set.upsert(policy)?;
    }
    Ok(set)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_defaults_match_legacy_baseline() {
        let set = builtin_defaults("deepseek");
        for id in [
            workload::ORGANIZER_STRUCTURED,
            workload::MEMORY_PLAN,
            workload::MEMORY_SYNTHESIZE,
            workload::MEMORY_JUDGE,
            workload::SCENE_JUDGE,
            workload::MINUTES_SUMMARIZE,
            workload::TRANSCRIPT_CORRECT,
        ] {
            let policy = set.resolve(id).expect("builtin workload must exist");
            assert_eq!(policy.candidates, vec!["deepseek".to_owned()], "{id}");
            assert!(!policy.allow_fallback, "{id}");
            assert_eq!(policy.max_attempts, 1, "{id}");
        }
        let deadlines = [
            (workload::ORGANIZER_STRUCTURED, 300_000u64),
            (workload::MEMORY_PLAN, 30_000),
            (workload::MEMORY_SYNTHESIZE, 30_000),
            (workload::MEMORY_JUDGE, 20_000),
            (workload::SCENE_JUDGE, 90_000),
            (workload::MINUTES_SUMMARIZE, 60_000),
            (workload::TRANSCRIPT_CORRECT, 30_000),
        ];
        for (id, ms) in deadlines {
            assert_eq!(set.resolve(id).unwrap().deadline_ms, ms, "{id}");
        }
        let structured = [
            workload::ORGANIZER_STRUCTURED,
            workload::MEMORY_PLAN,
            workload::SCENE_JUDGE,
            workload::TRANSCRIPT_CORRECT,
        ];
        for id in structured {
            assert!(
                set.resolve(id)
                    .unwrap()
                    .requires
                    .contains(&ProviderCapability::StructuredOutput),
                "{id} must require StructuredOutput"
            );
        }
    }

    #[test]
    fn resolve_unknown_workload_errors() {
        let set = builtin_defaults("deepseek");
        assert!(set.resolve("nope.workload").is_err());
    }

    #[test]
    fn upsert_rejects_invalid_policies() {
        let mut set = PolicySet::new();
        let mut policy = builtin_defaults("x")
            .resolve(workload::MEMORY_PLAN)
            .unwrap()
            .clone();
        policy.candidates.clear();
        assert!(matches!(
            set.upsert(policy.clone()),
            Err(PolicyError::EmptyCandidates(_))
        ));
        policy.candidates = vec!["a".to_owned()];
        policy.max_attempts = 0;
        assert!(matches!(
            set.upsert(policy),
            Err(PolicyError::InvalidMaxAttempts(_))
        ));
    }

    #[test]
    fn merge_overrides_replaces_only_matching_keys() {
        let builtins = builtin_defaults("deepseek");
        let mut overrides = PolicySet::new();
        overrides
            .upsert(WorkloadPolicy {
                workload: workload::ORGANIZER_STRUCTURED.to_owned(),
                candidates: vec!["deepseek".to_owned(), "pixel".to_owned()],
                allow_fallback: true,
                max_attempts: 3,
                deadline_ms: 120_000,
                requires: BTreeSet::from([ProviderCapability::StructuredOutput]),
            })
            .unwrap();
        let merged = merge_overrides(builtins, overrides);
        let organizer = merged.resolve(workload::ORGANIZER_STRUCTURED).unwrap();
        assert_eq!(organizer.candidates.len(), 2);
        assert!(organizer.allow_fallback);
        // 未覆盖项保留内置。
        assert_eq!(
            merged
                .resolve(workload::MINUTES_SUMMARIZE)
                .unwrap()
                .max_attempts,
            1
        );
    }

    #[test]
    fn overrides_file_roundtrip_and_errors() {
        let dir = std::env::temp_dir().join("model-runtime-policy-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("workloads.json");

        // 不存在 → 空。
        let _ = std::fs::remove_file(&path);
        assert!(load_overrides_file(&path)
            .unwrap()
            .workloads()
            .next()
            .is_none());

        // 合法样本。
        std::fs::write(
            &path,
            serde_json::json!({
                "version": 1,
                "workloads": [{
                    "workload": "organizer.structured",
                    "candidates": ["deepseek", "pixel"],
                    "allowFallback": true,
                    "maxAttempts": 3,
                    "deadlineMs": 120000,
                    "requires": ["structuredOutput"]
                }]
            })
            .to_string(),
        )
        .unwrap();
        let set = load_overrides_file(&path).unwrap();
        let policy = set.resolve(workload::ORGANIZER_STRUCTURED).unwrap();
        assert_eq!(policy.candidates, vec!["deepseek", "pixel"]);
        assert!(policy.allow_fallback);

        // 未知字段 → 拒绝。
        std::fs::write(
            &path,
            serde_json::json!({
                "version": 1,
                "workloads": [{
                    "workload": "organizer.structured",
                    "candidates": ["deepseek"],
                    "allowFallback": false,
                    "maxAttempts": 1,
                    "deadlineMs": 90000,
                    "requires": [],
                    "mystery": true
                }]
            })
            .to_string(),
        )
        .unwrap();
        assert!(load_overrides_file(&path).is_err());

        // 版本不符 → 拒绝。
        std::fs::write(
            &path,
            serde_json::json!({"version": 2, "workloads": []}).to_string(),
        )
        .unwrap();
        assert!(matches!(
            load_overrides_file(&path),
            Err(PolicyError::UnsupportedVersion(2))
        ));
        let _ = std::fs::remove_file(&path);
    }
}
