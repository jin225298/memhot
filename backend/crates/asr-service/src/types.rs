use serde::{Deserialize, Serialize};

// ASR transport must not depend on a physical microphone or desktop runtime.
pub use audio_contracts::AudioFormat;

/// ASR transcription result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptionResult {
    /// Transcribed text
    pub text: String,

    /// Confidence score (0.0 - 1.0)
    pub confidence: f32,

    /// Timestamp in milliseconds (relative to stream start)
    pub timestamp_ms: u64,

    /// Whether this is a final result (vs. interim/partial)
    pub is_final: bool,

    /// Provider-provided utterance boundaries and speaker labels. Providers
    /// without diarization support return one segment covering the result.
    #[serde(default)]
    pub segments: Vec<TranscriptionSegment>,

    /// Raw PCM for local voiceprint matching. This is an internal transport
    /// field and is intentionally omitted from serialized API responses.
    #[serde(skip)]
    pub audio_pcm: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TranscriptionSegment {
    pub text: String,
    pub start_ms: u64,
    pub end_ms: u64,
    #[serde(default)]
    pub speaker: Option<String>,
}

impl TranscriptionResult {
    pub fn new(text: String, confidence: f32, timestamp_ms: u64, is_final: bool) -> Self {
        Self {
            text,
            confidence,
            timestamp_ms,
            is_final,
            segments: Vec::new(),
            audio_pcm: None,
        }
    }
}
