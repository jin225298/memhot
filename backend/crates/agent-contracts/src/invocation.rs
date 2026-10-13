//! Provider-neutral accounting records shared by text and voice runtimes.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallKind {
    Text,
    Structured,
    Asr,
    Tts,
    Realtime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallStatus {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
    Interrupted,
}

/// Common identity for one logical provider call. It is safe to persist and
/// contains no credentials or message/audio payloads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCall {
    pub call_id: String,
    pub operation_id: Option<String>,
    pub session_id: Option<String>,
    pub workload: String,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub status: CallStatus,
    pub started_at_ms: i64,
    pub finished_at_ms: Option<i64>,
}

/// Voice call identity. The same record shape covers ASR, TTS and Realtime;
/// protocol-specific counters stay optional rather than being fabricated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceCall {
    pub call_id: String,
    pub operation_id: Option<String>,
    pub session_id: Option<String>,
    pub kind: CallKind,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub status: CallStatus,
    pub started_at_ms: i64,
    pub finished_at_ms: Option<i64>,
    pub input_duration_ms: Option<u64>,
    pub output_duration_ms: Option<u64>,
}

/// Unified usage/cost observation. Unknown values remain `None`; consumers
/// must not interpret missing usage as zero.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageRecord {
    pub call_id: String,
    pub kind: CallKind,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    pub input_audio_ms: Option<u64>,
    pub output_audio_ms: Option<u64>,
    pub request_id: Option<String>,
    pub status: CallStatus,
    pub observed_at_ms: i64,
    pub cost_micros_usd: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voice_usage_preserves_unknown_counters() {
        let record = UsageRecord {
            call_id: "voice-1".into(),
            kind: CallKind::Realtime,
            provider: Some("qwen".into()),
            model: None,
            prompt_tokens: None,
            completion_tokens: None,
            input_audio_ms: Some(1200),
            output_audio_ms: None,
            request_id: None,
            status: CallStatus::Completed,
            observed_at_ms: 1,
            cost_micros_usd: None,
        };
        let value = serde_json::to_value(record).unwrap();
        assert_eq!(value["inputAudioMs"], 1200);
        assert!(value["outputAudioMs"].is_null());
    }
}
