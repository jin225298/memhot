use crate::{
    db::Job,
    error::{Error, Result},
    hash, models, network, App,
};
use asr_service::{
    AsrError, AsrService, AudioFormat, MimoAsrService, MimoConfig, Qwen3AsrConfig, Qwen3AsrService,
};
use axum::http::StatusCode;
use rusqlite::params;
use serde_json::{json, Value};
use std::time::Duration;

pub struct Wav<'a> {
    pub format: AudioFormat,
    pub pcm: &'a [u8],
}
fn u16at(b: &[u8], n: usize) -> Result<u16> {
    Ok(u16::from_le_bytes(
        b.get(n..n + 2)
            .ok_or_else(|| Error::bad("WAV头不完整"))?
            .try_into()
            .map_err(|_| Error::bad("WAV头无效"))?,
    ))
}
fn u32at(b: &[u8], n: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(
        b.get(n..n + 4)
            .ok_or_else(|| Error::bad("WAV头不完整"))?
            .try_into()
            .map_err(|_| Error::bad("WAV头无效"))?,
    ))
}
pub fn parse_wav(data: &[u8]) -> Result<Wav<'_>> {
    if data.len() < 44
        || data.get(..4) != Some(b"RIFF")
        || data.get(8..12) != Some(b"WAVE")
        || u32at(data, 4)? as usize + 8 != data.len()
    {
        return Err(Error::bad("需要完整PCM WAV文件"));
    }
    let mut offset = 12;
    let mut format = None;
    let mut pcm = None;
    while offset + 8 <= data.len() {
        let size = u32at(data, offset + 4)? as usize;
        let start = offset + 8;
        let end = start
            .checked_add(size)
            .filter(|n| *n <= data.len())
            .ok_or_else(|| Error::bad("WAV数据块长度无效"))?;
        match &data[offset..offset + 4] {
            b"fmt " => {
                if size < 16 || u16at(data, start)? != 1 {
                    return Err(Error::bad("仅支持PCM WAV"));
                }
                let f = AudioFormat {
                    channels: u16at(data, start + 2)?,
                    sample_rate: u32at(data, start + 4)?,
                    bits_per_sample: u16at(data, start + 14)?,
                };
                if f.bits_per_sample != 16
                    || !(8000..=48000).contains(&f.sample_rate)
                    || !(1..=2).contains(&f.channels)
                {
                    return Err(Error::bad("需要8至48kHz、单/双声道、PCM16音频"));
                }
                let frame = u32::from(f.channels) * 2;
                if u16at(data, start + 12)? as u32 != frame
                    || u32at(data, start + 8)? != f.sample_rate * frame
                {
                    return Err(Error::bad("WAV采样参数不一致"));
                }
                format = Some(f);
            }
            b"data" => {
                if pcm.is_some() {
                    return Err(Error::bad("重复WAV数据块"));
                }
                pcm = Some(&data[start..end]);
            }
            _ => {}
        }
        offset = end + (size % 2);
    }
    let f = format.ok_or_else(|| Error::bad("WAV缺少fmt块"))?;
    let pcm = pcm.ok_or_else(|| Error::bad("WAV缺少data块"))?;
    let frame = usize::from(f.channels) * 2;
    if pcm.is_empty()
        || pcm.len() % frame != 0
        || pcm.len() as u64 > u64::from(f.sample_rate) * frame as u64 * 60
    {
        return Err(Error::bad("音频须为有效帧且不超过60秒"));
    }
    Ok(Wav { format: f, pcm })
}
pub fn wav(pcm: &[u8], format: AudioFormat) -> Vec<u8> {
    let mut out = Vec::with_capacity(pcm.len() + 44);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(pcm.len() as u32 + 36).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&format.channels.to_le_bytes());
    out.extend_from_slice(&format.sample_rate.to_le_bytes());
    out.extend_from_slice(&(format.sample_rate * u32::from(format.channels) * 2).to_le_bytes());
    out.extend_from_slice(&(format.channels * 2).to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    out.extend_from_slice(pcm);
    out
}
mod admission;
pub use admission::AudioAdmission;
mod ingest;
pub use ingest::{upload, upload_transcript};

fn asr_error(error: AsrError) -> Error {
    match error {
        AsrError::NetworkError(_) => {
            Error::upstream("network", "转写服务连接失败；结果不明时不自动重复付费")
        }
        AsrError::RateLimitExceeded => Error::upstream("rate_limited", "转写服务限流，请稍后重试"),
        AsrError::ServiceUnavailable => {
            Error::upstream("upstream", "转写服务暂时不可用，请稍后重试")
        }
        AsrError::TruncatedResponse => Error::upstream(
            "truncated",
            "转写响应不完整；本机音频已保留，请调整模型后明确重试",
        ),
        AsrError::InvalidResponse(_) => Error::upstream(
            "invalid_response",
            "转写服务未返回完整有效响应；本机音频已保留，请调整模型后明确重试",
        ),
        AsrError::AuthenticationError(_) => {
            Error::upstream("provider_auth", "转写服务凭据无效，请检查服务配置")
        }
        AsrError::ProviderError(_) => Error::upstream(
            "provider_rejected",
            "转写服务拒绝了此片段，请检查模型和音频格式",
        ),
        AsrError::UnsupportedFormat(_) => Error::bad("转写服务不支持此音频格式"),
        AsrError::Io(_) => Error::upstream("upstream", "转写服务暂时不可用，请稍后重试"),
    }
}
async fn transcribe(config: &Value, data: &[u8]) -> Result<String> {
    if !models::configured(config) {
        return Err(Error::new(
            StatusCode::CONFLICT,
            "model_unconfigured",
            "请配置ASR模型后重试转写",
        ));
    }
    let audio = parse_wav(data)?;
    let base = config["baseUrl"]
        .as_str()
        .unwrap_or_default()
        .trim_end_matches('/');
    let model = config["model"].as_str().unwrap_or_default();
    let key = config["apiKey"].as_str().unwrap_or_default();
    let client = network::asr_client_for(base).await?;
    let result = match config["protocol"].as_str().unwrap_or("openai") {
        "qwen" => Qwen3AsrService::with_client(
            Qwen3AsrConfig::new(key.into())
                .with_base_url(base)
                .with_model(model),
            client,
        )
        .transcribe_file(audio.pcm.to_vec(), audio.format)
        .await
        .map(|r| r.text)
        .map_err(asr_error),
        "mimo" => MimoAsrService::with_client(
            MimoConfig::new(key.into())
                .with_base_url(base)
                .with_model(model),
            client,
        )
        .transcribe_file(audio.pcm.to_vec(), audio.format)
        .await
        .map(|r| r.text)
        .map_err(asr_error),
        "openai" => {
            let part = reqwest::multipart::Part::bytes(data.to_vec())
                .file_name("segment.wav")
                .mime_str("audio/wav")
                .map_err(|_| Error::internal())?;
            let form = reqwest::multipart::Form::new()
                .text("model", model.to_owned())
                .part("file", part);
            let response = client
                .post(format!("{base}/audio/transcriptions"))
                .bearer_auth(key)
                .multipart(form)
                .send()
                .await
                .map_err(network::transport_error)?;
            if !response.status().is_success() {
                return Err(network::status_error(response.status()));
            }
            let value: Value =
                serde_json::from_slice(&network::bounded(response, network::MAX_BYTES).await?)
                    .map_err(|_| Error::upstream("invalid_response", "转写服务返回无效响应"))?;
            value["text"]
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| Error::upstream("invalid_response", "转写响应缺少有效text字段"))
        }
        _ => Err(Error::bad("不支持此ASR协议")),
    }?;
    Ok(result.trim().to_owned())
}
pub async fn process(app: &App, job: &Job) -> Result<Vec<(&'static str, Value)>> {
    let source_id = job.payload["sourceId"]
        .as_str()
        .ok_or_else(Error::missing)?;
    let mut source = app.store.get(&job.user, "source", source_id)?;
    source["status"] = json!("transcribing");
    let changed=app.store.conn()?.execute("UPDATE records SET data=? WHERE user_id=? AND kind='source' AND id=? AND EXISTS(SELECT 1 FROM jobs WHERE jobs.id=? AND jobs.user_id=? AND jobs.status='running')",params![source.to_string(),job.user,source_id,job.id,job.user])?;
    if changed == 0 {
        return Err(Error::missing());
    }
    let filename = source["_audioPath"].as_str().ok_or_else(Error::missing)?;
    if filename.contains('/') || filename.contains('\\') {
        return Err(Error::internal());
    }
    let _cleanup = LegacyAudioCleanup(app.data_dir.join("audio").join(filename));
    let data = tokio::fs::read(&_cleanup.0)
        .await
        .map_err(|_| Error::upstream("audio_missing", "待转写音频已过期或不可读取"))?;
    _cleanup.delete()?;
    source["audioDeleted"] = json!(true);
    source["audioRecovery"] = json!("reupload_same_segment");
    app.store.conn()?.execute(
        "UPDATE records SET data=? WHERE user_id=? AND kind='source' AND id=?",
        params![source.to_string(), job.user, source_id],
    )?;
    let config = models::config(app, &job.user, "asr")?;
    if !models::configured(&config) {
        return Err(Error::new(
            StatusCode::CONFLICT,
            "model_unconfigured",
            "请配置ASR模型，保存配置后会恢复转写",
        ));
    }
    let signature = hash(
        format!(
            "{}:asr:{}:{}:{}:{}",
            job.id,
            config["baseUrl"],
            config["protocol"],
            config["model"],
            hash(&data)
        )
        .as_bytes(),
    );
    let _permit = models::asr_permit(app, &job.user)?;
    let text = if let Some(text) =
        crate::ledger::reserve_units(app, job, &signature, "asr", asr_units(&data)?)?
    {
        text
    } else {
        let result = tokio::time::timeout(Duration::from_secs(100), transcribe(&config, &data))
            .await
            .unwrap_or_else(|_| Err(Error::upstream("timeout", "音频转写超时")));
        crate::ledger::settle(app, job, &signature, &result)?;
        result?
    };
    let empty = text.is_empty();
    source["text"] = json!(text);
    source["status"] = json!(if empty { "no_speech" } else { "ready" });
    source["audioDeleted"] = json!(true);
    source.as_object_mut().unwrap().remove("_audioPath");
    source["kind"] = json!("audio");
    source["analysisStatus"] = json!(if empty { "skipped" } else { "pending" });
    source["speakerMode"] = json!("unknown");
    let values = vec![("source", source)];
    Ok(values)
}
struct LegacyAudioCleanup(std::path::PathBuf);
impl LegacyAudioCleanup {
    fn delete(&self) -> Result<()> {
        match std::fs::remove_file(&self.0) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(Error::upstream(
                "audio_cleanup_failed",
                "无法清理临时音频，已停止转写，请检查服务器存储",
            )),
        }
    }
}
impl Drop for LegacyAudioCleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
fn asr_units(data: &[u8]) -> Result<i64> {
    let audio = parse_wav(data)?;
    let seconds = (audio.pcm.len() as u64)
        .div_ceil(u64::from(audio.format.sample_rate) * u64::from(audio.format.channels) * 2);
    Ok(seconds.div_ceil(30).max(1) as i64)
}
pub async fn test(app: &App, user: &str) -> Result<()> {
    test_idempotent(app, user, &crate::id()).await
}
pub async fn test_idempotent(app: &App, user: &str, request: &str) -> Result<()> {
    let config = models::config(app, user, "asr")?;
    let format = AudioFormat {
        sample_rate: 16000,
        channels: 1,
        bits_per_sample: 16,
    };
    let data = wav(&vec![0; 16000], format);
    let key = format!("asr-test:{request}");
    if !models::configured(&config) {
        return Err(Error::new(
            StatusCode::CONFLICT,
            "model_unconfigured",
            "请配置ASR模型后测试",
        ));
    }
    let _permit = models::asr_permit(app, user)?;
    if crate::ledger::reserve_request(app, user, None, &key, "asr", 1)?.is_some() {
        return Ok(());
    }
    let result = tokio::time::timeout(Duration::from_secs(100), transcribe(&config, &data))
        .await
        .unwrap_or_else(|_| Err(Error::upstream("timeout", "音频转写测试超时")));
    crate::ledger::settle_request(app, user, &key, &result)?;
    result.map(|_| ())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wav_parsing_removes_container_and_checks_frames() {
        let pcm = vec![0u8; 320];
        let f = AudioFormat {
            sample_rate: 16000,
            channels: 1,
            bits_per_sample: 16,
        };
        let data = wav(&pcm, f);
        let parsed = parse_wav(&data).unwrap();
        assert_eq!(parsed.pcm, &pcm);
        assert_eq!(parsed.format.sample_rate, 16000);
        let mut truncated = data.clone();
        truncated.pop();
        assert!(parse_wav(&truncated).is_err());
        let mut invalid = data;
        invalid[20] = 3;
        assert!(parse_wav(&invalid).is_err());
    }
    #[test]
    fn typed_asr_failures_preserve_retry_and_billing_semantics() {
        assert_eq!(
            asr_error(AsrError::AuthenticationError("private".into())).code,
            "provider_auth"
        );
        assert_eq!(
            asr_error(AsrError::ProviderError("private".into())).code,
            "provider_rejected"
        );
        assert_eq!(
            asr_error(AsrError::UnsupportedFormat("private".into())).code,
            "invalid_input"
        );
        assert_eq!(
            asr_error(AsrError::Io(std::io::Error::other("private"))).code,
            "upstream"
        );
        assert_eq!(asr_error(AsrError::RateLimitExceeded).code, "rate_limited");
        assert_eq!(asr_error(AsrError::ServiceUnavailable).code, "upstream");
        assert_eq!(
            asr_error(AsrError::NetworkError("private upstream body".into())).code,
            "network"
        );
        assert_eq!(asr_error(AsrError::TruncatedResponse).code, "truncated");
        assert_eq!(
            asr_error(AsrError::InvalidResponse("private response".into())).code,
            "invalid_response"
        );
    }
}
