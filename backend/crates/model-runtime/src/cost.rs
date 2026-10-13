//! Cost accounting for model attempts.
//!
//! Pricing is deliberately explicit and conservative: missing usage or a
//! missing price entry produces `Unknown`, never a fabricated zero cost.

use std::collections::{BTreeMap, VecDeque};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::catalog::ModelTarget;
use crate::contracts::{AttemptRecord, Usage, UsageRecord};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelPrice {
    /// Micro-USD per one million prompt tokens.
    pub prompt_micros_usd_per_million: u64,
    /// Micro-USD per one million completion tokens.
    pub completion_micros_usd_per_million: u64,
}

#[derive(Debug, Clone, Default)]
pub struct PriceTable {
    entries: BTreeMap<(String, String), ModelPrice>,
}

impl PriceTable {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(
        &mut self,
        provider: impl Into<String>,
        model: impl Into<String>,
        price: ModelPrice,
    ) {
        self.entries.insert((provider.into(), model.into()), price);
    }

    pub fn get(&self, provider: &str, model: &str) -> Option<ModelPrice> {
        self.entries
            .get(&(provider.to_owned(), model.to_owned()))
            .copied()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum CostState {
    Known { micros_usd: u64 },
    Unknown { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CostRecord {
    pub call_id: String,
    pub attempt_id: String,
    pub target_id: String,
    pub provider: String,
    pub model: String,
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    pub state: CostState,
}

#[derive(Debug, thiserror::Error)]
pub enum CostLedgerError {
    #[error("cost ledger I/O failed: {0}")]
    Io(String),
    #[error("cost ledger record is invalid: {0}")]
    InvalidRecord(String),
}

struct LedgerState {
    records: VecDeque<CostRecord>,
    usage_records: VecDeque<UsageRecord>,
    path: Option<PathBuf>,
}

/// Bounded, optionally durable cost ledger. The in-memory tail is bounded so
/// long-running voice sessions cannot accumulate unbounded accounting state.
pub struct CostLedger {
    prices: PriceTable,
    max_records: usize,
    state: Mutex<LedgerState>,
}

impl CostLedger {
    pub fn new(prices: PriceTable, max_records: usize) -> Self {
        Self {
            prices,
            max_records: max_records.max(1),
            state: Mutex::new(LedgerState {
                records: VecDeque::new(),
                usage_records: VecDeque::new(),
                path: None,
            }),
        }
    }

    pub fn open(
        path: impl AsRef<Path>,
        prices: PriceTable,
        max_records: usize,
    ) -> Result<Self, CostLedgerError> {
        let path = path.as_ref().to_path_buf();
        let mut records = VecDeque::new();
        if let Ok(raw) = fs::read_to_string(&path) {
            for line in raw.lines().filter(|line| !line.trim().is_empty()) {
                let record = serde_json::from_str(line)
                    .map_err(|error| CostLedgerError::InvalidRecord(error.to_string()))?;
                records.push_back(record);
            }
        }
        let max_records = max_records.max(1);
        while records.len() > max_records {
            records.pop_front();
        }
        Ok(Self {
            prices,
            max_records,
            state: Mutex::new(LedgerState {
                records,
                usage_records: VecDeque::new(),
                path: Some(path),
            }),
        })
    }

    pub fn records(&self) -> Vec<CostRecord> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .records
            .iter()
            .cloned()
            .collect()
    }

    pub fn usage_records(&self) -> Vec<UsageRecord> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .usage_records
            .iter()
            .cloned()
            .collect()
    }

    /// Record a provider-neutral usage observation alongside the cost ledger.
    /// Voice runtimes can use the same method even when token usage is absent.
    pub fn record_usage(&self, record: UsageRecord) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.usage_records.push_back(record);
        while state.usage_records.len() > self.max_records {
            state.usage_records.pop_front();
        }
    }

    pub fn record_attempts(
        &self,
        call_id: &str,
        attempts: &[AttemptRecord],
        targets: &[ModelTarget],
    ) -> Result<(), CostLedgerError> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut serialized = Vec::with_capacity(attempts.len());
        for attempt in attempts {
            let target = targets.iter().find(|target| target.id == attempt.target_id);
            let (provider, model) = target
                .map(|target| (target.provider.clone(), target.model.clone()))
                .unwrap_or_else(|| (String::new(), attempt.requested_model.clone()));
            let state_value = cost_state(&self.prices, target, attempt.usage, &attempt.status);
            let record = CostRecord {
                call_id: call_id.to_owned(),
                attempt_id: attempt.attempt_id.clone(),
                target_id: attempt.target_id.clone(),
                provider,
                model,
                prompt_tokens: attempt.usage.and_then(|usage| usage.prompt_tokens),
                completion_tokens: attempt.usage.and_then(|usage| usage.completion_tokens),
                state: state_value,
            };
            serialized.push(
                serde_json::to_string(&record)
                    .map_err(|error| CostLedgerError::InvalidRecord(error.to_string()))?,
            );
            state.records.push_back(record);
            while state.records.len() > self.max_records {
                state.records.pop_front();
            }
        }
        if let Some(path) = &state.path {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)
                    .map_err(|error| CostLedgerError::Io(error.to_string()))?;
            }
            let mut file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .map_err(|error| CostLedgerError::Io(error.to_string()))?;
            for line in serialized {
                writeln!(file, "{line}").map_err(|error| CostLedgerError::Io(error.to_string()))?;
            }
            file.sync_data()
                .map_err(|error| CostLedgerError::Io(error.to_string()))?;
        }
        Ok(())
    }
}

fn cost_state(
    prices: &PriceTable,
    target: Option<&ModelTarget>,
    usage: Option<Usage>,
    status: &crate::contracts::AttemptStatus,
) -> CostState {
    let Some(usage) = usage else {
        return CostState::Unknown {
            reason: "provider usage unavailable".to_owned(),
        };
    };
    let Some(target) = target else {
        return CostState::Unknown {
            reason: "target metadata unavailable".to_owned(),
        };
    };
    let Some(price) = prices.get(&target.provider, &target.model) else {
        return CostState::Unknown {
            reason: format!(
                "no price configured for {}/{}",
                target.provider, target.model
            ),
        };
    };
    if !matches!(status, crate::contracts::AttemptStatus::Completed) {
        return CostState::Unknown {
            reason: "attempt did not complete".to_owned(),
        };
    }
    let Some(prompt) = usage.prompt_tokens else {
        return CostState::Unknown {
            reason: "prompt token usage unavailable".to_owned(),
        };
    };
    let Some(completion) = usage.completion_tokens else {
        return CostState::Unknown {
            reason: "completion token usage unavailable".to_owned(),
        };
    };
    let micros = (u128::from(prompt) * u128::from(price.prompt_micros_usd_per_million)
        + u128::from(completion) * u128::from(price.completion_micros_usd_per_million))
        / 1_000_000;
    CostState::Known {
        micros_usd: micros.min(u128::from(u64::MAX)) as u64,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::{AttemptStatus, Usage};
    use std::collections::BTreeSet;

    fn target() -> ModelTarget {
        ModelTarget {
            id: "pixel".to_owned(),
            provider: "pixel".to_owned(),
            endpoint: "http://localhost/v1".to_owned(),
            model: "gpt-mini".to_owned(),
            revision: 1,
            capabilities: BTreeSet::new(),
        }
    }

    fn attempt(usage: Option<Usage>) -> AttemptRecord {
        AttemptRecord {
            attempt_id: "call#1".to_owned(),
            target_id: "pixel".to_owned(),
            target_revision: 1,
            requested_model: "gpt-mini".to_owned(),
            returned_model: None,
            provider_request_id: None,
            status: AttemptStatus::Completed,
            error_message: None,
            usage,
            latency_ms: 1,
        }
    }

    #[test]
    fn missing_usage_or_price_is_unknown_and_known_cost_is_integer() {
        let mut prices = PriceTable::new();
        prices.insert(
            "pixel",
            "gpt-mini",
            ModelPrice {
                prompt_micros_usd_per_million: 2_000_000,
                completion_micros_usd_per_million: 4_000_000,
            },
        );
        let ledger = CostLedger::new(prices, 4);
        ledger
            .record_attempts(
                "call",
                &[attempt(Some(Usage {
                    prompt_tokens: Some(1_000),
                    completion_tokens: Some(500),
                }))],
                &[target()],
            )
            .unwrap();
        assert_eq!(
            ledger.records()[0].state,
            CostState::Known { micros_usd: 4_000 }
        );
        ledger
            .record_attempts("call-unknown", &[attempt(None)], &[target()])
            .unwrap();
        assert!(matches!(
            ledger.records()[1].state,
            CostState::Unknown { .. }
        ));
    }

    #[test]
    fn memory_tail_is_bounded() {
        let ledger = CostLedger::new(PriceTable::new(), 1);
        ledger
            .record_attempts("a", &[attempt(None)], &[target()])
            .unwrap();
        ledger
            .record_attempts("b", &[attempt(None)], &[target()])
            .unwrap();
        assert_eq!(ledger.records().len(), 1);
        assert_eq!(ledger.records()[0].call_id, "b");
    }
}
