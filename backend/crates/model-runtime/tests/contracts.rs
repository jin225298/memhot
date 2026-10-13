//! contracts/catalog/policy 纯逻辑集成测试（Task 1 范围）。

use std::collections::BTreeSet;

use model_runtime::catalog::{self, ModelTarget};
use model_runtime::contracts::{CancelFlag, ChatTurn, ModelError, ModelErrorCode, OutputSpec};
use model_runtime::policy::{self, workload, PolicyError, PolicySet, WorkloadPolicy};
use provider_config::ProviderCapability;

#[test]
fn cancel_flag_is_shared_and_idempotent() {
    let flag = CancelFlag::new();
    let clone = flag.clone();
    assert!(!flag.is_cancelled());
    clone.cancel();
    assert!(flag.is_cancelled());
    flag.cancel(); // 幂等
    assert!(flag.is_cancelled());
}

#[test]
fn chat_turn_helpers_set_roles() {
    assert_eq!(ChatTurn::system("s").role, "system");
    assert_eq!(ChatTurn::user("u").role, "user");
    assert_eq!(ChatTurn::assistant("a").role, "assistant");
}

#[test]
fn model_error_carries_retry_after() {
    let error =
        ModelError::new(ModelErrorCode::RateLimited, "slow down", true).with_retry_after(1200);
    assert_eq!(error.retry_after_ms, Some(1200));
    assert!(error.retryable);
}

#[test]
fn output_spec_is_a_closed_set() {
    // v1 只有 Json/PlainText；编译期封闭，新增形态必须改契约并过评审。
    let _ = [OutputSpec::Json, OutputSpec::PlainText];
}

#[test]
fn synthetic_deepseek_declares_text_capabilities_as_unverified() {
    let target = ModelTarget::synthetic_deepseek(
        "https://api.deepseek.example".to_owned(),
        "deepseek-v4-flash".to_owned(),
    );
    assert_eq!(target.id, "deepseek");
    // bootstrap 合成 target 的 endpoint 也走归一化。
    assert_eq!(target.endpoint, "https://api.deepseek.example/v1");
    assert!(target.supports(&BTreeSet::from([ProviderCapability::StructuredOutput])));
    assert!(!target.supports(&BTreeSet::from([ProviderCapability::Embedding])));
}

#[test]
fn resolve_candidates_keeps_order_and_reports_skip_reasons() {
    let a = ModelTarget {
        id: "a".into(),
        provider: "deepseek".into(),
        endpoint: "https://a.example/v1".into(),
        model: "m".into(),
        revision: 1,
        capabilities: BTreeSet::from([ProviderCapability::StructuredOutput]),
    };
    let b = ModelTarget {
        id: "b".into(),
        capabilities: BTreeSet::from([ProviderCapability::ChatCompletion]),
        ..a.clone()
    };
    let resolved = catalog::resolve_candidates(
        &[a, b],
        &["b".to_owned(), "a".to_owned(), "missing".to_owned()],
        &BTreeSet::from([ProviderCapability::StructuredOutput]),
    );
    assert_eq!(resolved.len(), 3);
    assert!(resolved[0].1.as_deref().unwrap().contains("lacks required"));
    assert!(resolved[1].1.is_none());
    assert!(resolved[2].1.as_deref().unwrap().contains("not configured"));
}

#[test]
fn builtin_defaults_cover_all_workloads_with_legacy_semantics() {
    let set = policy::builtin_defaults("deepseek");
    let ids: Vec<&str> = set.workloads().map(|p| p.workload.as_str()).collect();
    assert_eq!(ids.len(), 11);
    assert!(ids.contains(&workload::ORGANIZER_STRUCTURED));
    assert!(ids.contains(&workload::MEMORY_PLAN));
    assert!(ids.contains(&workload::MEMORY_SYNTHESIZE));
    assert!(ids.contains(&workload::MEMORY_JUDGE));
    assert!(ids.contains(&workload::MEMORY02_FORMATION));
    assert!(ids.contains(&workload::MEMORY02_RECALL));
    assert!(ids.contains(&workload::SCENE_JUDGE));
    assert!(ids.contains(&workload::MINUTES_SUMMARIZE));
    assert!(ids.contains(&workload::TRANSCRIPT_CORRECT));
    assert!(ids.contains(&workload::PROJECT_ANSWER));
    assert!(ids.contains(&workload::GENERAL_CHAT));
    for p in set.workloads() {
        assert_eq!(p.max_attempts, 1, "{}", p.workload);
        assert!(!p.allow_fallback, "{}", p.workload);
        assert_eq!(p.candidates.len(), 1, "{}", p.workload);
    }
}

#[test]
fn overrides_must_fully_replace_builtin_entries() {
    let mut overrides = PolicySet::new();
    overrides
        .upsert(WorkloadPolicy {
            workload: workload::MINUTES_SUMMARIZE.to_owned(),
            candidates: vec!["pixel".to_owned()],
            allow_fallback: false,
            max_attempts: 2,
            deadline_ms: 45_000,
            requires: BTreeSet::from([ProviderCapability::ChatCompletion]),
        })
        .unwrap();
    let merged = policy::merge_overrides(policy::builtin_defaults("deepseek"), overrides);
    let minutes = merged.resolve(workload::MINUTES_SUMMARIZE).unwrap();
    assert_eq!(minutes.candidates, vec!["pixel"]);
    assert_eq!(minutes.max_attempts, 2);
    // 其他 workload 保留内置。
    assert_eq!(
        merged.resolve(workload::SCENE_JUDGE).unwrap().candidates,
        vec!["deepseek"]
    );
}

#[test]
fn policy_errors_are_specific() {
    let mut set = PolicySet::new();
    let bad = WorkloadPolicy {
        workload: String::new(),
        candidates: vec!["a".into()],
        allow_fallback: false,
        max_attempts: 1,
        deadline_ms: 1000,
        requires: BTreeSet::new(),
    };
    assert!(matches!(set.upsert(bad), Err(PolicyError::EmptyWorkload)));
    assert!(set.resolve("anything").is_err());
}
