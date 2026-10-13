use crate::{
    error::Result,
    types::{AudioFormat, TranscriptionResult},
};
use async_trait::async_trait;
use tokio::sync::mpsc;

/// ASR provider configuration
pub trait AsrConfig: Send + Sync + Clone {
    /// Get the API key or credentials
    fn credentials(&self) -> &str;

    /// Get the API endpoint URL
    fn endpoint(&self) -> &str;

    /// Get the language code (e.g., "zh-CN", "en-US")
    fn language(&self) -> &str;
}

/// ASR service interface
#[async_trait]
pub trait AsrService: Send + Sync {
    /// Start streaming ASR session
    ///
    /// # Arguments
    /// * `audio_rx` - Channel receiving `(segment_key, audio_chunk)` 对。`segment_key`
    ///   是语音段的单调标识（通常为累积语音毫秒数），用于把转写结果关联回说话人。
    /// * `text_tx` - Channel sending transcription results。结果的 `timestamp_ms`
    ///   会被设为该结果覆盖的最后一个语音段的 `segment_key`。
    /// * `format` - Audio format specification
    async fn start_stream(
        &self,
        audio_rx: mpsc::Receiver<(u64, Vec<u8>)>,
        text_tx: mpsc::Sender<TranscriptionResult>,
        format: AudioFormat,
    ) -> Result<()>;

    /// Transcribe a complete audio file
    async fn transcribe_file(
        &self,
        audio_data: Vec<u8>,
        format: AudioFormat,
    ) -> Result<TranscriptionResult>;
}
