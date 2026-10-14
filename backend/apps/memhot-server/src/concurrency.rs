//! Reserved capacity for interactive work and durable background jobs.
//! Waiting happens before billing. App clones share all gates; no cross-App statics.
use crate::error::{Error, Result};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, Weak},
    time::{Duration, Instant},
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Lane {
    Chat,
    Translation,
    Digest,
    Research,
}
impl Lane {
    pub const ALL: [Self; 4] = [Self::Chat, Self::Translation, Self::Digest, Self::Research];

    pub fn for_job(kind: &str) -> Self {
        match kind {
            "material_translation" => Self::Translation,
            "digest" => Self::Digest,
            _ => Self::Research,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Translation => "translation",
            Self::Digest => "digest",
            Self::Research => "research",
        }
    }
    fn index(self) -> usize {
        match self {
            Self::Chat => 0,
            Self::Translation => 1,
            Self::Digest => 2,
            Self::Research => 3,
        }
    }
}

struct Capacity {
    active: Arc<Semaphore>,
    admitted: Arc<Semaphore>,
}
impl Capacity {
    fn new() -> Self {
        Self {
            active: Arc::new(Semaphore::new(1)),
            // One executing call and at most sixteen queued owners per lane.
            admitted: Arc::new(Semaphore::new(17)),
        }
    }
}

type OwnerKey = (u8, Lane, String);
pub struct Concurrency {
    agents: [Capacity; 4],
    models: [Capacity; 4],
    owners: Mutex<HashMap<OwnerKey, Weak<Semaphore>>>,
}
impl Default for Concurrency {
    fn default() -> Self {
        Self {
            agents: std::array::from_fn(|_| Capacity::new()),
            models: std::array::from_fn(|_| Capacity::new()),
            owners: Mutex::new(HashMap::new()),
        }
    }
}

#[derive(Debug)]
pub struct Permit {
    _active: OwnedSemaphorePermit,
    _admitted: OwnedSemaphorePermit,
    _owner: Option<OwnedSemaphorePermit>,
}
impl Concurrency {
    pub async fn agent(&self, lane: Lane, owner: Option<&str>) -> Result<Permit> {
        self.acquire(true, lane, owner).await
    }
    pub async fn model(&self, lane: Lane, owner: Option<&str>) -> Result<Permit> {
        self.acquire(false, lane, owner).await
    }
    /// Cover history reads and final writes, not just the model/agent interval.
    pub fn conversation(&self, owner: &str) -> Result<OwnedSemaphorePermit> {
        self.owner(2, Lane::Chat, owner)
    }
    fn owner(&self, stage: u8, lane: Lane, owner: &str) -> Result<OwnedSemaphorePermit> {
        let mut owners = self.owners.lock().map_err(|_| Error::internal())?;
        owners.retain(|_, gate| gate.strong_count() > 0);
        let key = (stage, lane, owner.to_owned());
        let gate = if let Some(gate) = owners.get(&key).and_then(Weak::upgrade) {
            gate
        } else {
            if owners.len() >= 256 {
                return Err(busy());
            }
            let gate = Arc::new(Semaphore::new(1));
            owners.insert(key, Arc::downgrade(&gate));
            gate
        };
        // An owner may execute or wait once in each lane. Repeated requests cannot
        // occupy the queue ahead of other users or hold capacity while waiting.
        gate.try_acquire_owned()
            .map_err(|_| Error::upstream("busy", "同类请求正在处理，请等待完成后再试"))
    }
    async fn acquire(&self, agent: bool, lane: Lane, owner: Option<&str>) -> Result<Permit> {
        let owner = owner
            .map(|owner| self.owner(u8::from(agent), lane, owner))
            .transpose()?;
        let capacity = if agent { &self.agents } else { &self.models };
        let capacity = &capacity[lane.index()];
        let admitted = capacity
            .admitted
            .clone()
            .try_acquire_owned()
            .map_err(|_| busy())?;
        let start = Instant::now();
        // Tokio semaphores grant FIFO. Cancellation drops both the queued waiter
        // and owner permit, so a disconnected caller never leaves a phantom slot.
        let active = tokio::time::timeout(
            Duration::from_secs(30),
            capacity.active.clone().acquire_owned(),
        )
        .await
        .map_err(|_| busy())?
        .map_err(|_| Error::internal())?;
        eprintln!(
            "admission lane={} stage={} wait_ms={}",
            lane.name(),
            if agent { "agent" } else { "model" },
            start.elapsed().as_millis()
        );
        Ok(Permit {
            _active: active,
            _admitted: admitted,
            _owner: owner,
        })
    }
}

fn busy() -> Error {
    Error::upstream("busy", "同类任务较多，请稍后重试")
}

#[cfg(test)]
#[path = "concurrency_tests.rs"]
mod tests;
