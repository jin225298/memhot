use crate::{
    error::{Error, Result},
    network, App,
};
use serde_json::{json, Value};

/// Independent server-owned configuration. Never serialize this object to clients.
pub fn load_hosted() -> Result<Value> {
    let Some(path) = std::env::var_os("MEMHOT_PROVIDER_FILE") else {
        return Ok(json!({}));
    };
    let path = std::path::PathBuf::from(path);
    let meta = std::fs::symlink_metadata(&path).map_err(|_| Error::bad("无法读取托管模型配置"))?;
    if !meta.file_type().is_file() || meta.len() > 64 * 1024 {
        return Err(Error::bad("托管模型配置须为不超过64KiB的普通文件"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o777 != 0o600 {
            return Err(Error::bad("托管模型配置文件权限必须为0600"));
        }
    }
    let bytes = std::fs::read(&path).map_err(|_| Error::bad("无法读取托管模型配置"))?;
    let config: Value =
        serde_json::from_slice(&bytes).map_err(|_| Error::bad("托管模型配置JSON无效"))?;
    if !config.is_object() {
        return Err(Error::bad("托管模型配置须为JSON对象"));
    }
    for kind in ["text", "asr"] {
        if let Some(c) = config.get(kind) {
            if !super::configured(c) {
                return Err(Error::bad("托管模型配置缺少必要字段"));
            }
            let url = network::validate_url(c["baseUrl"].as_str().unwrap_or_default())?;
            if url.scheme() != "https" {
                return Err(Error::bad("托管模型地址必须使用HTTPS"));
            }
        }
    }
    Ok(config)
}
pub fn config(app: &App, user: &str, kind: &str) -> Result<Value> {
    if !matches!(kind, "text" | "asr") {
        return Err(Error::bad("模型类型无效"));
    }
    if let Some(config) = app.hosted.get(kind) {
        return Ok(config.clone());
    }
    Ok(app.store.profile(user)?["models"][kind].clone())
}
fn user_gate(
    app: &App,
    user: &str,
    lane: &str,
    limit: usize,
) -> Result<std::sync::Arc<tokio::sync::Semaphore>> {
    use std::{
        collections::HashMap,
        sync::{Arc, Mutex, OnceLock, Weak},
    };
    type Gates = HashMap<String, Weak<tokio::sync::Semaphore>>;
    static GATES: OnceLock<Mutex<Gates>> = OnceLock::new();
    Ok({
        let mut gates = GATES
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .map_err(|_| Error::internal())?;
        gates.retain(|_, gate| gate.strong_count() > 0);
        let key = format!("{}:{lane}:{user}", app.data_dir.to_string_lossy());
        if let Some(gate) = gates.get(&key).and_then(Weak::upgrade) {
            gate
        } else {
            let gate = Arc::new(tokio::sync::Semaphore::new(limit));
            gates.insert(key, Arc::downgrade(&gate));
            gate
        }
    })
}
pub type ModelPermits = (
    tokio::sync::OwnedSemaphorePermit,
    tokio::sync::OwnedSemaphorePermit,
);

/// Per-user concurrency is independent from durable unknown-result reservations.
pub async fn permit(app: &App, user: &str) -> Result<ModelPermits> {
    let user_permit = user_gate(app, user, "text", 2)?
        .try_acquire_owned()
        .map_err(|_| Error::upstream("busy", "你的其他请求正在处理，请稍后再试"))?;
    let global = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        app.model_gate.clone().acquire_owned(),
    )
    .await
    .map_err(|_| Error::upstream("busy", "模型正在处理其他任务，请稍后重试"))?
    .map_err(|_| Error::internal())?;
    Ok((user_permit, global))
}

/// Admit before reading a WAV body. Busy requests retain neither audio nor credits.
pub fn asr_permit(app: &App, user: &str) -> Result<ModelPermits> {
    let user = user_gate(app, user, "asr", 1)?
        .try_acquire_owned()
        .map_err(|_| Error::upstream("busy", "你的另一段录音正在同步，请稍后重试"))?;
    let global = app
        .asr_gate
        .clone()
        .try_acquire_owned()
        .map_err(|_| Error::upstream("busy", "语音转写正在处理其他片段，请稍后重试"))?;
    Ok((user, global))
}
