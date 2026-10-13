use crate::{
    error::{AsrError, Result},
    service::{AsrConfig, AsrService},
    types::{AudioFormat, TranscriptionResult, TranscriptionSegment},
};
use async_trait::async_trait;
use base64::Engine;
use reqwest::Client;
use serde::Serialize;
use serde_json::Value;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

const DEFAULT_ENDPOINT: &str = "https://dashscope.aliyuncs.com/compatible-mode/v1";
const DEFAULT_MODEL: &str = "qwen3-asr-flash";

#[derive(Clone)]
pub struct Qwen3AsrConfig {
    api_key: String,
    base_url: String,
    model: String,
    language: String,
    /// 热词列表（可选），格式为 {"词": 权重}
    hotwords: Option<std::collections::HashMap<String, i32>>,
}

impl std::fmt::Debug for Qwen3AsrConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Qwen3AsrConfig")
            .field("model", &self.model)
            .field("language", &self.language)
            .field("credentials", &"[redacted]")
            .finish_non_exhaustive()
    }
}

impl Qwen3AsrConfig {
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            base_url: DEFAULT_ENDPOINT.to_owned(),
            model: DEFAULT_MODEL.to_owned(),
            language: "zh".to_owned(),
            hotwords: None,
        }
    }

    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    pub fn with_language(mut self, language: impl Into<String>) -> Self {
        self.language = language.into();
        self
    }

    pub fn with_hotwords(mut self, hotwords: std::collections::HashMap<String, i32>) -> Self {
        self.hotwords = Some(hotwords);
        self
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    /// Load a Qwen section from the local API.md without exposing the key in
    /// source. The heading may be `Qwen3-ASR`, `阿里云Qwen3-ASR`, or similar.
    pub fn from_api_markdown(markdown: &str) -> Result<Self> {
        let mut in_section = false;
        let mut key = None;
        let mut base_url = None;
        let mut model = None;
        for line in markdown.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("###") {
                in_section = trimmed.to_ascii_lowercase().contains("qwen")
                    && trimmed.to_ascii_lowercase().contains("asr");
                continue;
            }
            if !in_section || trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            let Some((name, value)) = trimmed.split_once(':') else {
                continue;
            };
            let value = value.trim();
            match name.trim().to_ascii_lowercase().as_str() {
                "key" | "api_key" | "apikey" => key = Some(value.to_owned()),
                "base_url" | "endpoint" => base_url = Some(value.to_owned()),
                "model" => model = Some(value.to_owned()),
                _ => {}
            }
        }
        let key = key
            .filter(|value| !value.is_empty() && !value.contains('*'))
            .ok_or_else(|| {
                AsrError::AuthenticationError("Qwen3-ASR API.md section has no key".to_owned())
            })?;
        Ok(Self {
            api_key: key,
            base_url: base_url.unwrap_or_else(|| DEFAULT_ENDPOINT.to_owned()),
            model: model.unwrap_or_else(|| DEFAULT_MODEL.to_owned()),
            language: "zh".to_owned(),
            hotwords: None,
        })
    }
}

impl AsrConfig for Qwen3AsrConfig {
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

#[derive(Serialize)]
struct Request {
    model: String,
    messages: Vec<Message>,
    asr_options: AsrOptions,
    stream: bool,
}

#[derive(Serialize)]
struct Message {
    role: &'static str,
    content: Vec<Content>,
}

#[derive(Serialize)]
struct Content {
    #[serde(rename = "type")]
    kind: &'static str,
    input_audio: InputAudio,
}

#[derive(Serialize)]
struct InputAudio {
    data: String,
    format: &'static str,
}

#[derive(Serialize)]
struct AsrOptions {
    language: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    hotwords: Option<Vec<String>>,
}

pub struct Qwen3AsrService {
    config: Qwen3AsrConfig,
    client: Client,
}

impl Qwen3AsrService {
    /// Use a host-owned HTTP client, including its DNS pinning and timeout policy.
    pub fn with_client(config: Qwen3AsrConfig, client: Client) -> Self {
        Self { config, client }
    }

    pub fn new(config: Qwen3AsrConfig) -> Self {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(90))
            .build()
            .unwrap_or_else(|error| {
                error!("Qwen3-ASR client build failed: {}", error);
                Client::new()
            });
        Self { config, client }
    }

    async fn transcribe_window(
        &self,
        pcm: Vec<u8>,
        format: AudioFormat,
    ) -> Result<TranscriptionResult> {
        format
            .validate_pcm16(pcm.len())
            .map_err(|message| AsrError::UnsupportedFormat(message.into()))?;
        let wav = pcm_to_wav(&pcm, &format);
        let encoded = base64::engine::general_purpose::STANDARD.encode(wav);
        let request = Request {
            model: self.config.model.clone(),
            messages: vec![Message {
                role: "user",
                content: vec![Content {
                    kind: "input_audio",
                    input_audio: InputAudio {
                        data: format!("data:audio/wav;base64,{encoded}"),
                        format: "wav",
                    },
                }],
            }],
            asr_options: AsrOptions {
                language: self.config.language.clone(),
                hotwords: self
                    .config
                    .hotwords
                    .as_ref()
                    .map(|hw| hw.keys().cloned().collect()),
            },
            stream: false,
        };
        let mut retry = 0_u32;
        let response = loop {
            let response = self
                .client
                .post(format!(
                    "{}/chat/completions",
                    self.config.base_url.trim_end_matches('/')
                ))
                .bearer_auth(self.config.credentials())
                .json(&request)
                .send()
                .await
                .map_err(|error| AsrError::NetworkError(error.to_string()))?;
            if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS && retry < 3 {
                retry += 1;
                tokio::time::sleep(std::time::Duration::from_millis(500 * retry as u64)).await;
                continue;
            }
            break response;
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
        let payload: Value = crate::bounded_response::json(response).await?;
        crate::bounded_response::finish(
            payload
                .pointer("/choices/0/finish_reason")
                .and_then(Value::as_str),
        )?;
        let segments = parse_diarized_segments(&payload).unwrap_or_default();
        let text = response_text(&payload, &segments).ok_or_else(|| {
            AsrError::InvalidResponse("Qwen3-ASR response has no text".to_owned())
        })?;
        let mut result = TranscriptionResult::new(text, 1.0, 0, true);
        result.segments = segments;
        result.audio_pcm = Some(pcm);
        Ok(result)
    }
}

#[async_trait]
impl AsrService for Qwen3AsrService {
    async fn start_stream(
        &self,
        mut audio_rx: mpsc::Receiver<(u64, Vec<u8>)>,
        text_tx: mpsc::Sender<TranscriptionResult>,
        format: AudioFormat,
    ) -> Result<()> {
        info!("Starting Qwen3-ASR per-utterance streaming session");
        // VAD already produces single-speaker utterance candidates (up to 30s).
        // Transcribe each candidate immediately so local voiceprint matching and
        // timeline updates retain the same segment key and never collapse a
        // minute of multi-speaker audio into one record.
        while let Some((key, audio)) = audio_rx.recv().await {
            match self.transcribe_window(audio, format).await {
                Ok(mut result) => {
                    result.timestamp_ms = key;
                    if text_tx.send(result).await.is_err() {
                        break;
                    }
                }
                Err(_) => warn!("Qwen3-ASR segment {key} failed"),
            }
        }
        Ok(())
    }

    async fn transcribe_file(
        &self,
        audio_data: Vec<u8>,
        format: AudioFormat,
    ) -> Result<TranscriptionResult> {
        self.transcribe_window(audio_data, format).await
    }
}

pub fn parse_diarized_segments(payload: &Value) -> Option<Vec<TranscriptionSegment>> {
    let candidates = [
        payload.pointer("/output/sentence"),
        payload.pointer("/output/sentences"),
        payload.pointer("/output/results/0/sentences"),
        payload.pointer("/output/results/0/sentence"),
        payload.pointer("/sentences"),
    ];
    let items = candidates.into_iter().flatten().find_map(Value::as_array)?;
    let segments: Vec<_> = items.iter().filter_map(parse_segment).collect();
    (!segments.is_empty()).then_some(segments)
}

fn parse_segment(value: &Value) -> Option<TranscriptionSegment> {
    let text = value
        .get("text")
        .or_else(|| value.get("sentence"))?
        .as_str()?
        .trim();
    if text.is_empty() {
        return None;
    }
    let start_ms = time_field(value, &["begin_time", "start_time", "start_ms", "start"])?;
    let end_ms = time_field(value, &["end_time", "stop_time", "end_ms", "end"])?;
    let speaker = value
        .get("speaker_id")
        .or_else(|| value.get("speaker"))
        .map(|speaker| {
            speaker
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| format!("speaker-{}", speaker.as_i64().unwrap_or(0)))
        });
    Some(TranscriptionSegment {
        text: text.to_owned(),
        start_ms,
        end_ms,
        speaker,
    })
}

fn time_field(value: &Value, names: &[&str]) -> Option<u64> {
    names
        .iter()
        .find_map(|name| value.get(*name))
        .and_then(|raw| {
            raw.as_u64()
                .or_else(|| raw.as_f64().map(|v| (v * 1000.0) as u64))
                .or_else(|| {
                    raw.as_str()?
                        .parse::<f64>()
                        .ok()
                        .map(|v| (v * 1000.0) as u64)
                })
        })
}

pub(crate) fn response_text(payload: &Value, segments: &[TranscriptionSegment]) -> Option<String> {
    payload
        .pointer("/output/text")
        .and_then(content_text)
        .or_else(|| {
            payload
                .pointer("/choices/0/message/content")
                .and_then(content_text)
        })
        .or_else(|| {
            (!segments.is_empty()).then(|| {
                segments
                    .iter()
                    .map(|s| s.text.as_str())
                    .collect::<Vec<_>>()
                    .join("")
            })
        })
}

fn content_text(value: &Value) -> Option<String> {
    if let Some(text) = value.as_str() {
        return Some(text.to_owned());
    }
    let parts = value
        .as_array()?
        .iter()
        .filter_map(|item| {
            item.get("text")
                .and_then(Value::as_str)
                .or_else(|| item.get("content").and_then(Value::as_str))
        })
        .collect::<Vec<_>>();
    // A present, explicitly empty text is a valid no-speech result. Missing fields are not.
    (!parts.is_empty()).then(|| parts.join(""))
}

#[test]
fn explicit_empty_transcript_is_valid_but_missing_content_is_not() {
    assert_eq!(content_text(&serde_json::json!("")), Some(String::new()));
    assert_eq!(
        content_text(&serde_json::json!([{ "text": "" }])),
        Some(String::new())
    );
    assert_eq!(content_text(&serde_json::json!([])), None);
    assert_eq!(
        content_text(&serde_json::json!([{ "type": "audio" }])),
        None
    );
}

fn pcm_to_wav(pcm: &[u8], format: &AudioFormat) -> Vec<u8> {
    let sample_rate = format.sample_rate as u32;
    let channels = format.channels as u16;
    let bits = format.bits_per_sample as u16;
    let byte_rate = sample_rate * channels as u32 * (bits as u32 / 8);
    let block_align = channels * (bits / 8);
    let data_len = pcm.len() as u32;
    let mut wav = Vec::with_capacity(44 + pcm.len());
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_plan_config_uses_section_model_endpoint_and_key() {
        let markdown = r#"
### Qwen3-ASR
model: qwen3-asr-flash
base_url: https://workspace.cn-beijing.maas.aliyuncs.com/compatible-mode/v1
key: sk-test
"#;
        let config = Qwen3AsrConfig::from_api_markdown(markdown).expect("valid config");
        assert_eq!(config.credentials(), "sk-test");
        assert_eq!(
            config.endpoint(),
            "https://workspace.cn-beijing.maas.aliyuncs.com/compatible-mode/v1"
        );
        assert_eq!(config.model, "qwen3-asr-flash");
    }

    #[test]
    fn config_rejects_masked_or_missing_key_section() {
        let missing = "### Qwen3-ASR\nmodel: qwen3-asr-flash\n";
        assert!(Qwen3AsrConfig::from_api_markdown(missing).is_err());
        let masked = "### Qwen3-ASR\nkey: sk-ws-abcd****wxyz\n";
        assert!(Qwen3AsrConfig::from_api_markdown(masked).is_err());
    }

    #[test]
    fn compatible_response_content_array_is_joined() {
        let payload = serde_json::json!({
            "choices": [{"message": {"content": [
                {"type": "text", "text": "第一句。"},
                {"type": "text", "text": "第二句。"}
            ]}}]
        });
        assert_eq!(
            response_text(&payload, &[]).as_deref(),
            Some("第一句。第二句。")
        );
    }
}
