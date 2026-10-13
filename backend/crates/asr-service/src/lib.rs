mod bounded_response;
pub mod error;
pub mod filetrans;
pub mod hotwords;
pub mod mimo;
pub mod qwen;
pub mod service;
pub mod types;

pub use error::{AsrError, Result};
pub use filetrans::{
    parse_filetrans_segments, FiletransPoll, FiletransResult, FiletransSubmission,
    QwenFiletransConfig, QwenFiletransService,
};
pub use hotwords::{HotwordEntry, HotwordList};
pub use mimo::{MimoAsrService, MimoConfig};
pub use qwen::{Qwen3AsrConfig, Qwen3AsrService};
pub use service::{AsrConfig, AsrService};
pub use types::{AudioFormat, TranscriptionResult, TranscriptionSegment};

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mimo_config() {
        let config = MimoConfig::new("test-key".to_string());
        assert_eq!(config.credentials(), "test-key");
        assert_eq!(config.language(), "zh");
    }

    #[tokio::test]
    async fn test_transcription_result() {
        let result = TranscriptionResult::new("测试文本".to_string(), 0.95, 1000, true);
        assert_eq!(result.text, "测试文本");
        assert_eq!(result.confidence, 0.95);
        assert_eq!(result.timestamp_ms, 1000);
        assert!(result.is_final);
        assert!(result.segments.is_empty());
    }

    #[test]
    fn qwen_response_preserves_diarized_segments() {
        let response = serde_json::json!({
            "output": {
                "sentence": [
                    {"text": "你好", "begin_time": 0, "end_time": 420, "speaker_id": 0},
                    {"text": "请坐", "begin_time": 510, "end_time": 930, "speaker_id": 1}
                ]
            }
        });
        let segments = qwen::parse_diarized_segments(&response).expect("segments");
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].speaker.as_deref(), Some("speaker-0"));
        assert_eq!(segments[1].start_ms, 510);
    }
}
