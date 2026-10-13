//! Bootstrap：从旧 DeepSeek 配置链合成单 target runtime（过渡期装配）。
//!
//! Task 6 的完整 composition root（profile catalog + workloads 文件）就绪前，
//! host 与 capture-pipeline 共用本函数保持行为等价：单候选、内置默认 policy。

use std::sync::Arc;

use crate::catalog::{CatalogError, ModelConfigSource, ModelTarget};
use crate::execution::ModelRuntime;
use crate::policy;

struct SingleTargetSource {
    target: ModelTarget,
    credential: String,
}

impl ModelConfigSource for SingleTargetSource {
    fn list_targets(&self) -> Result<Vec<ModelTarget>, CatalogError> {
        Ok(vec![self.target.clone()])
    }

    fn credential(&self, target_id: &str) -> Result<Option<String>, CatalogError> {
        Ok((self.target.id == target_id).then(|| self.credential.clone()))
    }
}

/// 单 target runtime：内置默认 policy（单候选、单 attempt）。
pub fn single_target_runtime(target: ModelTarget, credential: String) -> Arc<ModelRuntime> {
    let policies = policy::builtin_defaults(&target.id);
    Arc::new(ModelRuntime::new(
        Arc::new(SingleTargetSource { target, credential }),
        policies,
    ))
}
