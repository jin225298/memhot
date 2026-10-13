use crate::{
    error::{AsrError, Result},
    service::{AsrConfig, AsrService},
    types::{AudioFormat, TranscriptionResult},
};
use async_trait::async_trait;
use base64::Engine;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

/// 小米 MIMO ASR 配置
#[derive(Clone)]
pub struct MimoConfig {
    api_key: String,
    base_url: String,
    model: String,
    language: String,
}

impl MimoConfig {
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            base_url: "https://api.xiaomimimo.com/v1".to_string(),
            model: "mimo-v2.5-asr".to_string(),
            language: "zh".to_string(),
        }
    }

    pub fn with_language(mut self, language: String) -> Self {
        self.language = language;
        self
    }

    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    pub fn model(&self) -> &str {
        &self.model
    }
}

impl AsrConfig for MimoConfig {
    fn credentials(&self) -> &str {
        &self.api_key
    }

    fn endpoint(&self) -> &str {
        &self.base_url
    }

    fn language(&self) -> &str {
        &self.language
    }
}

/// 小米 MIMO ASR 官方请求形态：OpenAI 协议兼容的 chat/completions，
/// 音频以 `input_audio` 消息（data URI）内嵌，`asr_options.language` 指定语言。
/// （非 `/audio/transcriptions` —— 那个路径 404。）
#[derive(Serialize)]
struct TranscriptionRequest {
    model: String,
    messages: Vec<AudioMessage>,
    asr_options: AsrOptions,
    stream: bool,
}

impl TranscriptionRequest {
    fn new(model: String, data_uri: String, language: &str) -> Self {
        Self {
            model,
            messages: vec![AudioMessage {
                role: "user".to_owned(),
                content: vec![AudioContent {
                    content_type: "input_audio".to_owned(),
                    input_audio: InputAudio {
                        data: data_uri,
                        format: "wav".to_owned(),
                    },
                }],
            }],
            asr_options: AsrOptions {
                language: language.to_owned(),
            },
            stream: false,
        }
    }
}

#[derive(Serialize)]
struct AudioMessage {
    role: String,
    content: Vec<AudioContent>,
}

#[derive(Serialize)]
struct AudioContent {
    #[serde(rename = "type")]
    content_type: String,
    input_audio: InputAudio,
}

#[derive(Serialize)]
struct InputAudio {
    data: String,
    format: String,
}

#[derive(Serialize)]
struct AsrOptions {
    language: String,
}

#[derive(Deserialize)]
struct TranscriptionResponse {
    #[serde(default)]
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    #[serde(default)]
    finish_reason: Option<String>,
    message: ResponseMessage,
}

#[derive(Deserialize)]
struct ResponseMessage {
    #[serde(default)]
    content: serde_json::Value,
}

/// 从 OpenAI 风格响应里取助手文本：`content` 可能是纯字符串，也可能是
/// `[{"type":"text","text":"…"}]` 数组（两种返回都兼容）。
fn extract_content_text(content: &serde_json::Value) -> Option<String> {
    if let Some(text) = content.as_str() {
        return Some(text.to_owned());
    }
    let parts = content
        .as_array()?
        .iter()
        .filter_map(|item| {
            item.get("text")
                .and_then(serde_json::Value::as_str)
                .or_else(|| item.get("content").and_then(serde_json::Value::as_str))
        })
        .collect::<Vec<_>>();
    (!parts.is_empty()).then(|| parts.join(""))
}

#[test]
fn explicit_empty_transcript_is_valid_but_missing_content_is_not() {
    assert_eq!(
        extract_content_text(&serde_json::json!("")),
        Some(String::new())
    );
    assert_eq!(
        extract_content_text(&serde_json::json!([{ "text": "" }])),
        Some(String::new())
    );
    assert_eq!(extract_content_text(&serde_json::json!([])), None);
    assert_eq!(
        extract_content_text(&serde_json::json!([{ "type": "audio" }])),
        None
    );
}

/// 小米 MIMO ASR 服务实现
pub struct MimoAsrService {
    config: MimoConfig,
    client: Client,
}

impl MimoAsrService {
    /// Use a host-owned HTTP client, including its DNS pinning and timeout policy.
    pub fn with_client(config: MimoConfig, client: Client) -> Self {
        Self { config, client }
    }

    pub fn new(config: MimoConfig) -> Self {
        // 60s 攒窗 + 转写本身耗时 → 请求超时放宽到 120s（与 voice-ingest 一致），
        // 避免长音频窗口在 ASR 正常耗时下被误判为超时。
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .unwrap_or_else(|e| {
                error!("MIMO client build failed: {}", e);
                Client::new()
            });
        Self { config, client }
    }
}

#[async_trait]
impl AsrService for MimoAsrService {
    async fn start_stream(
        &self,
        mut audio_rx: mpsc::Receiver<(u64, Vec<u8>)>,
        text_tx: mpsc::Sender<TranscriptionResult>,
        format: AudioFormat,
    ) -> Result<()> {
        info!("Starting MIMO ASR streaming session");

        // MIMO ASR 可能不支持真正的流式，这里模拟：累积语音片段、分批转写。
        // 与 voice-ingest 真实 provider 对齐（60s 窗口）——3s 窗口会在一分钟内
        // 打出 ~20 次请求触发 MiMo 限流（HTTP 429）；60s 窗口把 23min 音频压到
        // ~24 次请求，处于限流配额内。
        let chunk_duration_ms = 60_000; // 每 60 秒转写一次
        let mut windower = Windower::new(&format, chunk_duration_ms);

        let mut window_count = 0u64;
        while let Some((segment_key, audio_chunk)) = audio_rx.recv().await {
            if let Some((batch_key, to_transcribe)) = windower.push(segment_key, &audio_chunk) {
                window_count += 1;
                info!(
                    "MIMO ASR window {} (key {}) transcribing {} bytes (~{:.1}s audio)...",
                    window_count,
                    batch_key,
                    to_transcribe.len(),
                    to_transcribe.len() as f64
                        / (format.sample_rate as f64
                            * format.channels as f64
                            * format.bits_per_sample as f64
                            / 8.0)
                );
                match self.transcribe_file(to_transcribe, format).await {
                    Ok(mut result) => {
                        // 用段 key 标记该结果覆盖的最后一个语音段，供说话人关联
                        result.timestamp_ms = batch_key;
                        info!(
                            "MIMO ASR window {} done → {} chars",
                            window_count,
                            result.text.chars().count()
                        );
                        if text_tx.send(result).await.is_err() {
                            info!("Receiver closed, stopping stream");
                            break;
                        }
                    }
                    Err(_) => {
                        error!("Transcription error (window {})", window_count);
                    }
                }
            }
        }

        // 流结束后，转写剩余不足一个窗口的音频
        if let Some((batch_key, to_transcribe)) = windower.flush() {
            window_count += 1;
            info!(
                "MIMO ASR flush tail (key {}) transcribing {} bytes (~{:.1}s audio)...",
                batch_key,
                to_transcribe.len(),
                to_transcribe.len() as f64
                    / (format.sample_rate as f64
                        * format.channels as f64
                        * format.bits_per_sample as f64
                        / 8.0)
            );
            match self.transcribe_file(to_transcribe, format).await {
                Ok(mut result) => {
                    result.timestamp_ms = batch_key;
                    info!(
                        "MIMO ASR flush done → {} chars",
                        result.text.chars().count()
                    );
                    let _ = text_tx.send(result).await;
                }
                Err(_) => {
                    error!("Transcription error (flush)");
                }
            }
        }

        info!(
            "MIMO ASR streaming session ended ({} windows total)",
            window_count
        );
        Ok(())
    }

    async fn transcribe_file(
        &self,
        audio_data: Vec<u8>,
        format: AudioFormat,
    ) -> Result<TranscriptionResult> {
        format
            .validate_pcm16(audio_data.len())
            .map_err(|message| AsrError::UnsupportedFormat(message.into()))?;
        debug!("Transcribing audio: {} bytes", audio_data.len());

        // MiMo `input_audio` 以 data URI 内嵌完整 WAV 文件（含 RIFF 头），
        // 这里把窗口缓冲的裸 PCM 包成 16-bit PCM WAV。
        let wav_bytes = pcm_to_wav(&audio_data, &format);
        let audio_base64 = base64::engine::general_purpose::STANDARD.encode(&wav_bytes);

        let request = TranscriptionRequest::new(
            self.config.model.clone(),
            format!("data:audio/wav;base64,{audio_base64}"),
            &self.config.language,
        );

        // 限流重试：MiMo 对短时的请求频率有限流（HTTP 429）。直接用 3s 攒窗
        // 会密集打满限额；这里收窄窗口到 60s 之外，再对偶发 429 做最多 3 次、
        // 500ms 递增退避的重试（与 voice-ingest 的策略一致），避免丢整段音频。
        const MAX_RETRIES: u32 = 3;
        let mut attempt = 0u32;
        let response = loop {
            let resp = self
                .client
                .post(format!(
                    "{}/chat/completions",
                    self.config.endpoint().trim_end_matches('/')
                ))
                .header(
                    "Authorization",
                    format!("Bearer {}", self.config.credentials()),
                )
                .header("Content-Type", "application/json")
                .json(&request)
                .send()
                .await
                .map_err(|e| AsrError::NetworkError(e.to_string()))?;

            if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS && attempt < MAX_RETRIES {
                attempt += 1;
                let backoff_ms = 500 * attempt as u64;
                warn!("MiMo ASR 限流(429)，{backoff_ms}ms 后重试 {attempt}/{MAX_RETRIES}");
                tokio::time::sleep(std::time::Duration::from_millis(backoff_ms)).await;
                continue;
            }
            break resp;
        };

        if !response.status().is_success() {
            let status = response.status();
            return Err(if status.as_u16() == 401 || status.as_u16() == 403 {
                AsrError::AuthenticationError(format!("HTTP {status}"))
            } else if status.as_u16() == 429 {
                AsrError::RateLimitExceeded
            } else if status.is_server_error() {
                AsrError::ServiceUnavailable
            } else {
                AsrError::ProviderError(format!("HTTP {status}"))
            });
        }

        let transcription: TranscriptionResponse = crate::bounded_response::json(response).await?;

        crate::bounded_response::finish(
            transcription
                .choices
                .first()
                .and_then(|c| c.finish_reason.as_deref()),
        )?;
        let text = transcription
            .choices
            .first()
            .and_then(|choice| extract_content_text(&choice.message.content))
            .ok_or_else(|| AsrError::InvalidResponse("MiMo 响应缺少助手文本".to_owned()))?;

        debug!(
            "Transcription complete: {} characters",
            text.chars().count()
        );

        Ok(TranscriptionResult::new(
            text, 1.0, 0, // MIMO doesn't provide timestamp in this mode
            true,
        ))
    }
}

/// 把裸 PCM 字节包成标准 WAV（16-bit PCM），供 MiMo `input_audio` 使用。
fn pcm_to_wav(pcm: &[u8], format: &AudioFormat) -> Vec<u8> {
    let sample_rate = format.sample_rate as u32;
    let channels = format.channels as u16;
    let bits = format.bits_per_sample as u16;
    let byte_rate = sample_rate * channels as u32 * (bits as u32 / 8);
    let block_align = channels * (bits as u32 / 8) as u16;
    let data_len = pcm.len() as u32;

    let mut wav = Vec::with_capacity(44 + pcm.len());
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVE");
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&channels.to_le_bytes());
    wav.extend_from_slice(&sample_rate.to_le_bytes());
    wav.extend_from_slice(&byte_rate.to_le_bytes());
    wav.extend_from_slice(&block_align.to_le_bytes());
    wav.extend_from_slice(&bits.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    wav.extend_from_slice(pcm);
    wav
}

/// 把语音片段按时间窗口累积成「待转写批次」。
///
/// 每次 `push` 传入 `(segment_key, audio)`；当窗口内累积的语音时长达到
/// `chunk_duration_ms` 时产出一次 `(batch_key, batch_audio)`，其中 `batch_key`
/// 是该窗口内最后一个语音段的 key。窗口在产出后**清空**，保证后续转写只覆盖新音频
/// （避免原实现把全部历史音频反复转写）。
struct Windower {
    bytes_per_ms: u64,
    chunk_duration_ms: u64,
    buffer: Vec<u8>,
    pending_ms: u64,
    last_key: u64,
}

impl Windower {
    fn new(format: &AudioFormat, chunk_duration_ms: u64) -> Self {
        let bytes_per_ms =
            (format.sample_rate as u64 * format.channels as u64 * format.bits_per_sample as u64)
                / (8 * 1000);
        Self {
            bytes_per_ms: bytes_per_ms.max(1),
            chunk_duration_ms,
            buffer: Vec::new(),
            pending_ms: 0,
            last_key: 0,
        }
    }

    /// 累积一个语音段；窗口已满时返回待转写批次。
    fn push(&mut self, segment_key: u64, audio: &[u8]) -> Option<(u64, Vec<u8>)> {
        self.last_key = segment_key;
        self.buffer.extend_from_slice(audio);
        self.pending_ms += audio.len() as u64 / self.bytes_per_ms;
        if self.pending_ms >= self.chunk_duration_ms {
            let batch = std::mem::take(&mut self.buffer);
            self.pending_ms = 0;
            Some((self.last_key, batch))
        } else {
            None
        }
    }

    /// 流结束时取走剩余不足一个窗口的音频。
    fn flush(&mut self) -> Option<(u64, Vec<u8>)> {
        if self.buffer.is_empty() {
            return None;
        }
        let batch = std::mem::take(&mut self.buffer);
        self.pending_ms = 0;
        Some((self.last_key, batch))
    }
}

#[cfg(test)]
#[path = "mimo_tests.rs"]
mod tests;
