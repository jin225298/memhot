pub mod accounts;
pub mod agent;
mod agent_structured;
pub mod agent_web;
pub mod aihot;
pub mod alphaxiv;
pub mod api;
pub mod audio;
pub mod auth;
pub mod billing;
pub mod cognition;
pub mod daily;
pub mod db;
pub mod digest;
pub mod discovery;
pub mod discussion;
mod discussion_requests;
pub mod editorial;
pub mod error;
pub mod feed;
pub mod formation;
mod formation_output;
mod formation_quotes;
mod formation_repair;
mod formation_windows;
pub mod freshness;
pub mod import;
pub mod ledger;
pub mod mcp_transport;
pub mod models;
pub mod network;
mod persistence;
pub mod preferences_setup;
pub mod profile;
pub mod public_usage;
pub mod reading;
pub mod report_access;
mod report_copyedit;
pub mod research;
mod research_claims;
mod research_evidence;
mod research_prompts;
mod research_quality;
mod research_queue;
mod research_reasoning;
pub mod retry;
mod search_backend;
pub mod shared_public;
pub mod sources;
pub mod thought_list;
pub mod topics;
pub mod worker;

use chrono::{SecondsFormat, Utc};
use sha2::{Digest, Sha256};
use std::{path::PathBuf, sync::Arc};

#[derive(Clone)]
pub struct App {
    pub store: Arc<db::Store>,
    pub data_dir: Arc<PathBuf>,
    pub model_gate: Arc<tokio::sync::Semaphore>,
    pub asr_gate: Arc<tokio::sync::Semaphore>,
    pub hosted: Arc<serde_json::Value>,
    pub alphaxiv: Arc<Option<alphaxiv::Config>>,
}
impl App {
    pub fn open(dir: PathBuf) -> error::Result<Self> {
        std::fs::create_dir_all(&dir).map_err(|_| error::Error::internal())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
                .map_err(|_| error::Error::internal())?;
        }
        Ok(Self {
            store: Arc::new(db::Store::open(&dir.join("memhot.sqlite"))?),
            data_dir: Arc::new(dir),
            model_gate: Arc::new(tokio::sync::Semaphore::new(3)),
            asr_gate: Arc::new(tokio::sync::Semaphore::new(3)),
            hosted: Arc::new(models::load_hosted()?),
            alphaxiv: Arc::new(alphaxiv::Config::load()?),
        })
    }
}
pub fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}
pub fn id() -> String {
    let mut bytes = [0u8; 20];
    getrandom::fill(&mut bytes).expect("OS randomness is required");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
pub fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub fn clip(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}
