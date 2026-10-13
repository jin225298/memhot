use crate::{qwen::response_text, AsrError, Result, TranscriptionSegment};
use base64::Engine;
use reqwest::Client;
use serde_json::{json, Value};
use std::path::Path;

const DEFAULT_ENDPOINT: &str = "https://dashscope.aliyuncs.com/compatible-mode/v1";
const DEFAULT_MODEL: &str = "qwen3-asr-flash";
const LEGACY_COMPATIBLE_MODEL: &str = "qwen-audio-3.0-asr-flash-filetrans";

#[derive(Clone, Debug)]
pub struct QwenFiletransConfig {
    api_key: String,
    base_url: String,
    model: String,
    language: String,
    /// 热词列表（可选）
    hotwords: Option<std::collections::HashMap<String, i32>>,
}

impl QwenFiletransConfig {
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            base_url: DEFAULT_ENDPOINT.to_owned(),
            model: DEFAULT_MODEL.to_owned(),
            language: "zh".to_owned(),
            hotwords: None,
        }
    }

    pub fn from_api_markdown(markdown: &str) -> Result<Self> {
        let mut in_section = false;
        let mut key = None;
        let mut base_url = None;
        let mut filetrans_base_url = None;
        let mut model = None;
        let mut filetrans_model = None;
        for line in markdown.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("###") {
                let heading = trimmed.to_ascii_lowercase();
                in_section = heading.contains("asr")
                    && (heading.contains("qwen")
                        || heading.contains("recording")
                        || heading.contains("mimo"));
                continue;
            }
            if !in_section || trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            let Some((name, value)) = trimmed.split_once(':') else {
                continue;
            };
            let value = value.trim().to_owned();
            match name.trim().to_ascii_lowercase().as_str() {
                "key" | "api_key" | "apikey" => key = Some(value),
                "base_url" | "endpoint" => base_url = Some(value),
                "filetrans_base_url" | "filetrans_endpoint" => filetrans_base_url = Some(value),
                "model" => model = Some(value),
                "filetrans_model" => filetrans_model = Some(value),
                _ => {}
            }
        }
        let key = key
            .filter(|value| !value.is_empty() && !value.contains('*'))
            .ok_or_else(|| {
                AsrError::AuthenticationError("ASR configuration section has no key".to_owned())
            })?;
        // A Filetrans model is valid only with its dedicated endpoint. Pairing
        // them prevents an old native model name from leaking into compatible
        // mode, where it is rejected before any audio can be transcribed.
        let filetrans_base_url = filetrans_base_url.filter(|value| !value.is_empty());
        let use_filetrans_endpoint = filetrans_base_url.is_some();
        let configured_model = if use_filetrans_endpoint {
            filetrans_model.or(model)
        } else {
            model.filter(|value| !value.eq_ignore_ascii_case(LEGACY_COMPATIBLE_MODEL))
        };
        Ok(Self {
            api_key: key,
            base_url: filetrans_base_url
                .or(base_url)
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| DEFAULT_ENDPOINT.to_owned()),
            model: configured_model.unwrap_or_else(|| DEFAULT_MODEL.to_owned()),
            language: "zh".to_owned(),
            hotwords: None,
        })
    }

    pub fn with_base_url(mut self, value: impl Into<String>) -> Self {
        self.base_url = value.into();
        self
    }

    pub fn with_model(mut self, value: impl Into<String>) -> Self {
        self.model = value.into();
        self
    }

    pub fn with_hotwords(mut self, hotwords: std::collections::HashMap<String, i32>) -> Self {
        self.hotwords = Some(hotwords);
        self
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn endpoint(&self) -> &str {
        &self.base_url
    }

    pub fn credentials(&self) -> &str {
        &self.api_key
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct FiletransResult {
    pub segments: Vec<TranscriptionSegment>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum FiletransSubmission {
    Completed(FiletransResult),
    Pending {
        task_id: String,
        poll_url: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum FiletransPoll {
    Processing,
    Completed(FiletransResult),
    Failed(String),
}

pub struct QwenFiletransService {
    config: QwenFiletransConfig,
    client: Client,
}

impl QwenFiletransService {
    pub fn new(config: QwenFiletransConfig) -> Self {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .unwrap_or_else(|_| Client::new());
        Self { config, client }
    }

    pub async fn submit_wav(&self, path: &Path) -> Result<FiletransSubmission> {
        let wav = tokio::fs::read(path).await?;
        let audio_duration_ms = wav_duration_ms(&wav)?;
        let encoded = base64::engine::general_purpose::STANDARD.encode(wav);
        let data_url = format!("data:audio/wav;base64,{encoded}");
        let native = self
            .config
            .base_url
            .contains("/services/audio/asr/transcription");
        let endpoint = if native {
            self.config.base_url.clone()
        } else if self.config.base_url.ends_with("/chat/completions") {
            self.config.base_url.clone()
        } else {
            format!(
                "{}/chat/completions",
                self.config.base_url.trim_end_matches('/')
            )
        };
        let body = if native {
            let mut params = json!({
                "language_hints": [self.config.language],
                "diarization_enabled": true
            });
            if let Some(hotwords) = &self.config.hotwords {
                if !hotwords.is_empty() {
                    let hotword_list: Vec<String> = hotwords
                        .iter()
                        .map(|(word, weight)| format!("{}:{}", word, weight))
                        .collect();
                    params["hotwords"] = json!(hotword_list);
                }
            }
            json!({
                "model": self.config.model,
                "input": { "file_urls": [data_url] },
                "parameters": params
            })
        } else {
            let asr_options = json!({
                "language": self.config.language
            });
            let mut body = json!({
                "model": self.config.model,
                "messages": [{
                    "role": "user",
                    "content": [{
                        "type": "input_audio",
                        "input_audio": { "data": data_url, "format": "wav" }
                    }]
                }],
                "asr_options": asr_options,
                "stream": false
            });
            if let Some(hotwords) = &self.config.hotwords {
                let mut words: Vec<_> = hotwords
                    .iter()
                    .filter(|(_, weight)| **weight >= 0)
                    .map(|(word, _)| word.as_str())
                    .collect();
                words.sort();
                if !words.is_empty() {
                    body["messages"]
                        .as_array_mut()
                        .unwrap()
                        .insert(0, json!({"role": "system", "content": words.join("、")}));
                }
            }
            body
        };
        let mut request = self
            .client
            .post(endpoint)
            .bearer_auth(&self.config.api_key)
            .json(&body);
        if native {
            request = request.header("X-DashScope-Async", "enable");
        }
        let response = request
            .send()
            .await
            .map_err(|error| AsrError::NetworkError(error.to_string()))?;
        let payload = checked_json(response).await?;
        parse_submission_with_audio_duration(&payload, audio_duration_ms)
    }

    pub async fn poll_task(&self, task_id: &str, poll_url: Option<&str>) -> Result<FiletransPoll> {
        let url = poll_url
            .map(str::to_owned)
            .unwrap_or_else(|| self.default_poll_url(task_id));
        let response = self
            .client
            .get(url)
            .bearer_auth(&self.config.api_key)
            .send()
            .await
            .map_err(|error| AsrError::NetworkError(error.to_string()))?;
        let payload = checked_json(response).await?;
        if let Some(result_url) = string_at(
            &payload,
            &[
                "/output/results/0/transcription_url",
                "/output/transcription_url",
                "/transcription_url",
            ],
        ) {
            let response = self
                .client
                .get(result_url)
                .send()
                .await
                .map_err(|error| AsrError::NetworkError(error.to_string()))?;
            let result_payload = checked_json(response).await?;
            return Ok(FiletransPoll::Completed(FiletransResult {
                segments: parse_filetrans_segments(&result_payload)?,
            }));
        }
        parse_poll(&payload)
    }

    fn default_poll_url(&self, task_id: &str) -> String {
        if let Ok(mut url) = reqwest::Url::parse(&self.config.base_url) {
            url.set_path(&format!("/api/v1/tasks/{task_id}"));
            url.set_query(None);
            return url.to_string();
        }
        format!(
            "{}/tasks/{task_id}",
            self.config.base_url.trim_end_matches('/')
        )
    }
}

async fn checked_json(response: reqwest::Response) -> Result<Value> {
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| AsrError::NetworkError(error.to_string()))?;
    if !status.is_success() {
        return Err(if status.as_u16() == 401 || status.as_u16() == 403 {
            AsrError::AuthenticationError(format!("HTTP {status}: {body}"))
        } else {
            AsrError::ProviderError(format!("HTTP {status}: {body}"))
        });
    }
    serde_json::from_str(&body)
        .map_err(|error| AsrError::ProviderError(format!("invalid Qwen response: {error}")))
}

pub fn parse_submission(payload: &Value) -> Result<FiletransSubmission> {
    parse_submission_with_audio_duration(payload, 0)
}

fn parse_submission_with_audio_duration(
    payload: &Value,
    audio_duration_ms: u64,
) -> Result<FiletransSubmission> {
    if let Ok(segments) = parse_filetrans_segments(payload) {
        return Ok(FiletransSubmission::Completed(FiletransResult { segments }));
    }
    if audio_duration_ms > 0 {
        if let Some(text) = response_text(payload, &[])
            .map(|text| text.trim().to_owned())
            .filter(|text| !text.is_empty())
        {
            return Ok(FiletransSubmission::Completed(FiletransResult {
                segments: vec![TranscriptionSegment {
                    text,
                    start_ms: 0,
                    end_ms: audio_duration_ms,
                    speaker: None,
                }],
            }));
        }
    }
    // 异步任务流：提交响应含 task_id → 转入轮询。
    if let Some(task_id) = string_at(payload, &["/output/task_id", "/task_id"]) {
        let poll_url = string_at(
            payload,
            &[
                "/output/task_url",
                "/output/result_url",
                "/task_url",
                "/result_url",
            ],
        );
        return Ok(FiletransSubmission::Pending { task_id, poll_url });
    }
    // 200 响应既无 task_id 也无句子文本：常驻环境录音中的纯噪声/无人声块
    // 属于正常情况——按空结果处理（不产生记录、不报错）。把「没有语音」
    // 当成转写失败会让桌宠在安静环境中持续弹「环境记录处理异常」。
    Ok(FiletransSubmission::Completed(FiletransResult {
        segments: Vec::new(),
    }))
}

fn wav_duration_ms(wav: &[u8]) -> Result<u64> {
    if wav.get(0..4) != Some(b"RIFF") || wav.get(8..12) != Some(b"WAVE") {
        return Err(AsrError::UnsupportedFormat(
            "Filetrans input is not a RIFF/WAVE file".to_owned(),
        ));
    }
    let mut offset = 12_usize;
    let mut byte_rate = None;
    let mut data_size = None;
    while offset.checked_add(8).is_some_and(|end| end <= wav.len()) {
        let id = &wav[offset..offset + 4];
        let size = u32::from_le_bytes(
            wav[offset + 4..offset + 8]
                .try_into()
                .expect("WAV chunk size has four bytes"),
        ) as usize;
        let data_start = offset + 8;
        let data_end = data_start.checked_add(size).ok_or_else(|| {
            AsrError::UnsupportedFormat("Filetrans WAV chunk size overflowed".to_owned())
        })?;
        if data_end > wav.len() {
            return Err(AsrError::UnsupportedFormat(
                "Filetrans WAV contains a truncated chunk".to_owned(),
            ));
        }
        if id == b"fmt " && size >= 12 {
            byte_rate = Some(u32::from_le_bytes(
                wav[data_start + 8..data_start + 12]
                    .try_into()
                    .expect("WAV byte rate has four bytes"),
            ));
        } else if id == b"data" {
            data_size = Some(size as u64);
        }
        offset = data_end.saturating_add(size % 2);
    }
    let byte_rate = u64::from(byte_rate.filter(|value| *value > 0).ok_or_else(|| {
        AsrError::UnsupportedFormat("Filetrans WAV has no valid byte rate".to_owned())
    })?);
    let data_size = data_size.ok_or_else(|| {
        AsrError::UnsupportedFormat("Filetrans WAV has no audio data chunk".to_owned())
    })?;
    let duration_ms = data_size
        .saturating_mul(1_000)
        .saturating_add(byte_rate - 1)
        / byte_rate;
    if duration_ms == 0 {
        return Err(AsrError::UnsupportedFormat(
            "Filetrans WAV contains no audio frames".to_owned(),
        ));
    }
    Ok(duration_ms)
}

pub fn parse_poll(payload: &Value) -> Result<FiletransPoll> {
    if let Ok(segments) = parse_filetrans_segments(payload) {
        return Ok(FiletransPoll::Completed(FiletransResult { segments }));
    }
    let status = string_at(payload, &["/output/task_status", "/task_status", "/status"])
        .unwrap_or_default()
        .to_ascii_uppercase();
    match status.as_str() {
        "PENDING" | "QUEUED" | "RUNNING" | "PROCESSING" => Ok(FiletransPoll::Processing),
        "FAILED" | "CANCELED" | "CANCELLED" => Ok(FiletransPoll::Failed(
            string_at(payload, &["/output/message", "/message", "/error/message"])
                .unwrap_or_else(|| format!("provider task ended with status {status}")),
        )),
        _ => Err(AsrError::ProviderError(format!(
            "Filetrans poll response has no sentences and unknown status '{status}'"
        ))),
    }
}

pub fn parse_filetrans_segments(payload: &Value) -> Result<Vec<TranscriptionSegment>> {
    let candidates = [
        "/output/sentence",
        "/output/sentences",
        "/output/results/0/sentences",
        "/output/results/0/sentence",
        "/output/results/0/transcription/sentences",
        "/transcripts/0/sentences",
        "/transcription/transcripts/0/sentences",
        "/result/sentences",
        "/sentences",
    ];
    let items = candidates
        .iter()
        .find_map(|pointer| payload.pointer(pointer).and_then(Value::as_array))
        .ok_or_else(|| {
            AsrError::ProviderError("Filetrans response has no sentence array".to_owned())
        })?;
    let mut segments = Vec::with_capacity(items.len());
    for (index, value) in items.iter().enumerate() {
        let text = value
            .get("text")
            .or_else(|| value.get("sentence"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| AsrError::ProviderError(format!("sentence {index} has no text")))?;
        let start_ms = segment_time(value, true).ok_or_else(|| {
            AsrError::ProviderError(format!("sentence {index} has no start time"))
        })?;
        let end_ms = segment_time(value, false)
            .ok_or_else(|| AsrError::ProviderError(format!("sentence {index} has no end time")))?;
        let speaker = value
            .get("speaker_id")
            .or_else(|| value.get("speaker"))
            .and_then(speaker_label);
        segments.push(TranscriptionSegment {
            text: text.to_owned(),
            start_ms,
            end_ms,
            speaker,
        });
    }
    if segments.is_empty() {
        return Err(AsrError::ProviderError(
            "Filetrans sentence array is empty".to_owned(),
        ));
    }
    Ok(segments)
}

fn segment_time(value: &Value, start: bool) -> Option<u64> {
    let millisecond_names: &[&str] = if start {
        &["begin_time", "start_ms"]
    } else {
        &["end_time", "stop_time", "end_ms"]
    };
    for name in millisecond_names {
        if let Some(raw) = value.get(*name) {
            return number(raw, false);
        }
    }
    let second_names: &[&str] = if start {
        &["start_seconds", "begin_seconds"]
    } else {
        &["end_seconds", "stop_seconds"]
    };
    for name in second_names {
        if let Some(raw) = value.get(*name) {
            return number(raw, true);
        }
    }
    let generic_names: &[&str] = if start {
        &["start_time", "start"]
    } else {
        &["end_time", "end"]
    };
    for name in generic_names {
        if let Some(raw) = value.get(*name) {
            let seconds = raw.as_f64().is_some_and(|number| number.fract() != 0.0)
                || raw.as_str().is_some_and(|text| text.contains('.'));
            return number(raw, seconds);
        }
    }
    None
}

fn number(raw: &Value, seconds: bool) -> Option<u64> {
    let value = raw
        .as_f64()
        .or_else(|| raw.as_str()?.trim().parse::<f64>().ok())?;
    if !value.is_finite() || value < 0.0 {
        return None;
    }
    Some(if seconds {
        (value * 1000.0).round() as u64
    } else {
        value.round() as u64
    })
}

fn speaker_label(value: &Value) -> Option<String> {
    if let Some(value) = value
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Some(value.to_owned());
    }
    value.as_i64().map(|value| format!("speaker-{value}"))
}

fn string_at(payload: &Value, pointers: &[&str]) -> Option<String> {
    pointers.iter().find_map(|pointer| {
        let value = payload.pointer(pointer)?;
        value
            .as_str()
            .map(str::to_owned)
            .or_else(|| value.as_i64().map(|value| value.to_string()))
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn recording_asr_preserves_mimo_profile() {
        let config = super::QwenFiletransConfig::from_api_markdown(
            "### Recording-ASR\nmodel: mimo-v2.5-asr\nbase_url: https://api.xiaomimimo.com/v1\nkey: test-mimo-key\n",
        ).unwrap();
        assert_eq!(config.model(), "mimo-v2.5-asr");
        assert_eq!(config.endpoint(), "https://api.xiaomimimo.com/v1");
        assert_eq!(config.credentials(), "test-mimo-key");
    }

    use super::*;

    #[test]
    fn compatible_config_keeps_the_configured_qwen_asr_model() {
        let config = QwenFiletransConfig::from_api_markdown(
            "### Qwen3-ASR\nmodel: qwen3-asr-flash\nbase_url: https://example.test/v1\nkey: sk-test\n",
        )
        .unwrap();
        assert_eq!(config.model(), "qwen3-asr-flash");
        assert_eq!(config.endpoint(), "https://example.test/v1");
    }

    #[test]
    fn filetrans_model_without_its_endpoint_cannot_override_compatible_mode() {
        let config = QwenFiletransConfig::from_api_markdown(
            "### Qwen3-ASR\nmodel: qwen3-asr-flash\nbase_url: https://example.test/compatible-mode/v1\nfiletrans_model: qwen-audio-3.0-asr-flash-filetrans\nkey: sk-test\n",
        )
        .unwrap();

        assert_eq!(config.model(), "qwen3-asr-flash");
        assert_eq!(config.endpoint(), "https://example.test/compatible-mode/v1");
    }

    #[test]
    fn legacy_filetrans_model_is_replaced_in_compatible_mode() {
        let config = QwenFiletransConfig::from_api_markdown(
            "### Qwen3-ASR\nmodel: qwen-audio-3.0-asr-flash-filetrans\nbase_url: https://example.test/compatible-mode/v1\nkey: sk-test\n",
        )
        .unwrap();

        assert_eq!(config.model(), DEFAULT_MODEL);
    }

    #[test]
    fn explicit_filetrans_settings_still_override_compatible_fallback() {
        let config = QwenFiletransConfig::from_api_markdown(
            "### Qwen3-ASR\nmodel: qwen3-asr-flash\nbase_url: https://example.test/v1\nfiletrans_model: qwen-filetrans-custom\nfiletrans_base_url: https://native.example.test/services/audio/asr/transcription\nkey: sk-test\n",
        )
        .unwrap();

        assert_eq!(config.model(), "qwen-filetrans-custom");
        assert_eq!(
            config.endpoint(),
            "https://native.example.test/services/audio/asr/transcription"
        );
    }

    #[test]
    fn compatible_completion_text_becomes_a_full_duration_segment() {
        let payload = json!({
            "choices": [{"message": {"content": [
                {"type": "text", "text": "第一句。"},
                {"type": "text", "text": "第二句。"}
            ]}}]
        });

        let submission = parse_submission_with_audio_duration(&payload, 2_500).unwrap();

        assert!(matches!(
            submission,
            FiletransSubmission::Completed(FiletransResult { segments })
                if segments == vec![TranscriptionSegment {
                    text: "第一句。第二句。".to_owned(),
                    start_ms: 0,
                    end_ms: 2_500,
                    speaker: None,
                }]
        ));
    }

    #[test]
    fn wav_duration_uses_fmt_byte_rate_and_data_size() {
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36_u32 + 64_000).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16_u32.to_le_bytes());
        wav.extend_from_slice(&1_u16.to_le_bytes());
        wav.extend_from_slice(&1_u16.to_le_bytes());
        wav.extend_from_slice(&16_000_u32.to_le_bytes());
        wav.extend_from_slice(&32_000_u32.to_le_bytes());
        wav.extend_from_slice(&2_u16.to_le_bytes());
        wav.extend_from_slice(&16_u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&64_000_u32.to_le_bytes());
        wav.resize(44 + 64_000, 0);

        assert_eq!(wav_duration_ms(&wav).unwrap(), 2_000);
    }

    #[test]
    fn parser_preserves_speakers_and_handles_explicit_time_units() {
        let payload = json!({"output": {"results": [{"sentences": [
            {"text": "第一句", "begin_time": 120, "end_time": 900, "speaker_id": 0},
            {"text": "第二句", "start_seconds": 1.25, "end_seconds": "2.5", "speaker": "B"}
        ]}]}});
        let segments = parse_filetrans_segments(&payload).unwrap();
        assert_eq!(segments[0].start_ms, 120);
        assert_eq!(segments[0].speaker.as_deref(), Some("speaker-0"));
        assert_eq!(segments[1].start_ms, 1_250);
        assert_eq!(segments[1].end_ms, 2_500);
        assert_eq!(segments[1].speaker.as_deref(), Some("B"));
    }

    #[test]
    fn parser_rejects_missing_required_sentence_fields() {
        let payload = json!({"sentences": [{"text": "", "start_ms": 0, "end_ms": 5}]});
        assert!(parse_filetrans_segments(&payload).is_err());
        let payload = json!({"sentences": [{"text": "hello", "start_ms": 0}]});
        assert!(parse_filetrans_segments(&payload).is_err());
    }

    #[test]
    fn submission_and_poll_support_async_and_direct_envelopes() {
        let pending = parse_submission(&json!({"output": {
            "task_id": "task-1", "task_status": "PENDING", "task_url": "https://example.test/task-1"
        }}))
        .unwrap();
        assert!(
            matches!(pending, FiletransSubmission::Pending { task_id, .. } if task_id == "task-1")
        );
        assert_eq!(
            parse_poll(&json!({"output": {"task_status": "RUNNING"}})).unwrap(),
            FiletransPoll::Processing
        );
        assert!(matches!(
            parse_submission(&json!({"sentences": [{
                "text": "done", "start_ms": 0, "end_ms": 100
            }]}))
            .unwrap(),
            FiletransSubmission::Completed(_)
        ));
    }
}
