//! ModelTarget catalog：从现有 provider profile 派生候选 target。
//!
//! v1 不新建配置：profile 仍是唯一配置来源；host 侧 bootstrap 把
//! env/API.md/.secrets 旧链合成为 target。能力状态只有 Declared（来自
//! profile 声明）与 Unknown（bootstrap 合成），不虚构 Verified。

use std::collections::BTreeSet;

use provider_config::ProviderCapability;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CatalogError {
    #[error("config source unavailable: {0}")]
    Source(String),
}

/// 一个可调用的模型目标：profile revision + endpoint + model + 能力声明。
/// 同一模型名在不同服务端是不同 target。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelTarget {
    /// v1 = provider profile id；bootstrap 合成 target 沿用 `deepseek`。
    pub id: String,
    /// 供应商标识（如 `deepseek`）；adapter flavor 判定用。
    pub provider: String,
    /// 已归一化的 chat-completions base_url（含 `/v1`）。
    pub endpoint: String,
    pub model: String,
    pub revision: u64,
    /// 声明的能力（Declared，未经行为验证）。
    pub capabilities: BTreeSet<ProviderCapability>,
}

impl ModelTarget {
    /// bootstrap 合成 target：声明文本与结构化能力，未经实测。
    pub fn synthetic_deepseek(endpoint: String, model: String) -> Self {
        Self {
            id: "deepseek".to_owned(),
            provider: "deepseek".to_owned(),
            endpoint: normalize_chat_base_url(&endpoint),
            model,
            revision: 1,
            capabilities: BTreeSet::from([
                ProviderCapability::ChatCompletion,
                ProviderCapability::StructuredOutput,
            ]),
        }
    }

    /// 从 provider profile 派生（endpoint 归一化与 profile revision 冻结）。
    pub fn from_profile(profile: &provider_config::ProviderProfile) -> Self {
        Self {
            id: profile.id.as_str().to_owned(),
            provider: profile.settings.provider.clone(),
            endpoint: normalize_chat_base_url(&profile.settings.endpoint),
            model: profile.settings.model.clone(),
            revision: profile.revision,
            capabilities: profile.settings.capabilities.clone(),
        }
    }

    pub fn supports(&self, required: &BTreeSet<ProviderCapability>) -> bool {
        required.is_subset(&self.capabilities)
    }
}

/// 配置来源端口：host 实现于 ProviderViewStore/ProviderConfigService 之上；
/// 测试用静态实现。凭据只在 attempt 执行时解析，不进快照、不进记录。
#[async_trait::async_trait]
pub trait ModelConfigSource: Send + Sync {
    fn list_targets(&self) -> Result<Vec<ModelTarget>, CatalogError>;

    /// 返回 target 的 API key；未配置凭据返回 Ok(None)（调用时报 Auth）。
    fn credential(&self, target_id: &str) -> Result<Option<String>, CatalogError>;
}

/// 归一化 chat-completions base_url：去尾 `/`；既不以版本路径
///（`/v1`、`/v2`、`/api/v3` 等）也不以 `/chat/completions` 结尾时补 `/v1`。
/// OpenAI 兼容网关缺版本路径时请求会打到网关首页返回 HTML；火山方舟的
/// `/api/v3` 也必须保留，不能再追加一个 `/v1`。
///
/// 自 organizer-service/src/config.rs 迁入，本函数是唯一权威实现。
pub fn normalize_chat_base_url(base_url: &str) -> String {
    let trimmed = base_url.trim_end_matches('/');
    if has_version_path(trimmed) || trimmed.ends_with("/chat/completions") {
        trimmed.to_owned()
    } else {
        format!("{trimmed}/v1")
    }
}

fn has_version_path(base_url: &str) -> bool {
    let Some(last_segment) = base_url.rsplit('/').next() else {
        return false;
    };
    let Some(version) = last_segment.strip_prefix('v') else {
        return false;
    };
    !version.is_empty() && version.chars().all(|character| character.is_ascii_digit())
}

/// 按候选顺序保留存在的 target，并标记缺能力的候选（返回 (target, skip_reason)）。
/// 缺能力候选保留位置与原因（记 Skipped attempt），不静默消失。
pub fn resolve_candidates(
    targets: &[ModelTarget],
    candidate_ids: &[String],
    required: &BTreeSet<ProviderCapability>,
) -> Vec<(ModelTarget, Option<String>)> {
    let mut resolved = Vec::new();
    for id in candidate_ids {
        let Some(target) = targets.iter().find(|t| &t.id == id) else {
            // 未配置的候选 ID：保留占位记录，便于诊断"配置里写了但没配好"。
            resolved.push((
                ModelTarget {
                    id: id.clone(),
                    provider: String::new(),
                    endpoint: String::new(),
                    model: String::new(),
                    revision: 0,
                    capabilities: BTreeSet::new(),
                },
                Some(format!("candidate `{id}` is not configured")),
            ));
            continue;
        };
        if target.supports(required) {
            resolved.push((target.clone(), None));
        } else {
            resolved.push((
                target.clone(),
                Some(format!("candidate `{id}` lacks required capabilities")),
            ));
        }
    }
    resolved
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(id: &str, caps: &[ProviderCapability]) -> ModelTarget {
        ModelTarget {
            id: id.to_owned(),
            provider: "deepseek".to_owned(),
            endpoint: "https://x.example/v1".to_owned(),
            model: "m".to_owned(),
            revision: 3,
            capabilities: caps.iter().copied().collect(),
        }
    }

    #[test]
    fn normalize_appends_v1_only_when_missing() {
        assert_eq!(normalize_chat_base_url("https://x.com"), "https://x.com/v1");
        assert_eq!(
            normalize_chat_base_url("https://x.com/"),
            "https://x.com/v1"
        );
        assert_eq!(
            normalize_chat_base_url("https://x.com/v1"),
            "https://x.com/v1"
        );
        assert_eq!(
            normalize_chat_base_url("https://x.com/v1/"),
            "https://x.com/v1"
        );
        assert_eq!(
            normalize_chat_base_url("https://x.com/chat/completions"),
            "https://x.com/chat/completions"
        );
        assert_eq!(
            normalize_chat_base_url("https://ark.cn-beijing.volces.com/api/v3/"),
            "https://ark.cn-beijing.volces.com/api/v3"
        );
    }

    #[test]
    fn resolve_candidates_marks_missing_and_unqualified() {
        let caps = [ProviderCapability::StructuredOutput];
        let targets = vec![
            target("a", &caps),
            target("b", &[ProviderCapability::ChatCompletion]),
        ];
        let ids = vec!["a".to_owned(), "b".to_owned(), "c".to_owned()];
        let resolved = resolve_candidates(
            &targets,
            &ids,
            &BTreeSet::from([ProviderCapability::StructuredOutput]),
        );
        assert_eq!(resolved.len(), 3);
        assert!(resolved[0].1.is_none());
        assert!(resolved[1].1.as_deref().unwrap().contains("lacks required"));
        assert!(resolved[2].1.as_deref().unwrap().contains("not configured"));
    }
}
