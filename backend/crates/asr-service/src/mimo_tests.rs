use super::*;

fn fmt16k() -> AudioFormat {
    AudioFormat::standard_speech() // 16kHz mono 16bit → 32 bytes/ms
}

/// 生成 n 毫秒的单声道 16bit 静音音频。
fn audio(ms: u64) -> Vec<u8> {
    vec![0u8; (ms * 32) as usize]
}

#[test]
fn windower_emits_when_window_full() {
    let mut w = Windower::new(&fmt16k(), 3000);
    // 2 个 2s 段：第一次不足窗口，第二次凑够 4s ≥ 3s
    assert!(w.push(1000, &audio(2000)).is_none());
    let (key, batch) = w.push(2000, &audio(2000)).unwrap();
    assert_eq!(key, 2000, "batch_key 应取窗口内最后一个段的 key");
    assert_eq!(batch.len(), audio(4000).len());
}

#[test]
fn windower_emits_exactly_once_per_window() {
    let mut w = Windower::new(&fmt16k(), 3000);
    let mut count = 0;
    for i in 0..6u64 {
        if w.push(i * 1000, &audio(2000)).is_some() {
            count += 1;
        }
    }
    // 累积 12s / 3s 窗口 → 3 次产出（每 2 段 4s 填满一个 3s 窗口）
    assert_eq!(count, 3);
}

#[test]
fn windower_flush_drains_remainder() {
    let mut w = Windower::new(&fmt16k(), 3000);
    // 单段 1s，不足窗口
    assert!(w.push(500, &audio(1000)).is_none());
    let (key, batch) = w.flush().unwrap();
    assert_eq!(key, 500);
    assert_eq!(batch.len(), audio(1000).len());
    // 再次 flush 无剩余
    assert!(w.flush().is_none());
}

#[test]
fn windower_empty_flush_is_none() {
    let mut w = Windower::new(&fmt16k(), 3000);
    assert!(w.flush().is_none());
}

#[test]
fn windower_resets_buffer_after_emit() {
    let mut w = Windower::new(&fmt16k(), 3000);
    // 第一段填满窗口并产出，只含第一段音频
    let (k1, b1) = w.push(1, &audio(3000)).unwrap();
    assert_eq!(k1, 1);
    assert_eq!(b1.len(), audio(3000).len());
    // 第二段再次填满窗口，只含第二段音频（第一段已被取走）
    let (k2, b2) = w.push(2, &audio(3000)).unwrap();
    assert_eq!(k2, 2);
    assert_eq!(
        b2.len(),
        audio(3000).len(),
        "产出后缓冲必须清空，不能带历史音频"
    );
    // 缓冲已空，无剩余可 flush
    assert!(w.flush().is_none());
}

#[test]
fn pcm_to_wav_writes_valid_riff_header() {
    let format = AudioFormat::standard_speech();
    let pcm = vec![0u8; 640]; // 20ms @16k mono
    let wav = pcm_to_wav(&pcm, &format);
    assert_eq!(&wav[0..4], b"RIFF");
    assert_eq!(&wav[8..12], b"WAVE");
    assert_eq!(&wav[12..16], b"fmt ");
    assert_eq!(&wav[36..40], b"data");
    assert_eq!(wav.len(), 44 + pcm.len());
    // fmt 块：PCM(1) / mono(1) / 16kHz / 16bit
    assert_eq!(&wav[20..22], &1u16.to_le_bytes()); // audio_format = PCM
    assert_eq!(&wav[22..24], &1u16.to_le_bytes()); // channels = 1
    assert_eq!(&wav[24..28], &16_000u32.to_le_bytes()); // sample rate
    assert_eq!(&wav[28..32], &32_000u32.to_le_bytes()); // byte rate = 16000*1*2
    assert_eq!(&wav[32..34], &2u16.to_le_bytes()); // block align
    assert_eq!(&wav[34..36], &16u16.to_le_bytes()); // bits
}

// 真实 MiMo ASR 冒烟：用 `.secrets/API.md` 的 key 转写 wav 的一小段。
// 默认 `#[ignore]`（网络成本），端到端测试前先单独跑它确认端点形态。
//
// ```bash
// cargo test -p asr-service mimo_real_smoke -- --ignored --nocapture
// ```
#[tokio::test]
#[ignore = "真实 MiMo API（需 .secrets/API.md 密钥 + wav 音频）"]
async fn mimo_real_smoke() {
    const SECRETS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../.secrets/API.md");
    const WAV: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../experiments/realtime-e2e/audio/test_30min_v2.wav"
    );
    let markdown = std::fs::read_to_string(SECRETS).expect("读取 .secrets/API.md");
    let key = extract_section_key(&markdown, "小米MIMO-ASR");
    // 取 wav 前 5 秒语音
    let data = std::fs::read(WAV).expect("读 wav");
    let mut pcm = Vec::new();
    let mut off = 12usize;
    while off + 8 <= data.len() {
        let id = &data[off..off + 4];
        let size = u32::from_le_bytes([data[off + 4], data[off + 5], data[off + 6], data[off + 7]])
            as usize;
        if id == b"data" {
            pcm = data[off + 8..(off + 8 + size).min(data.len())].to_vec();
        }
        off += 8 + size + (size % 2);
    }
    // 依次测试不同音频时长（5s / 15s / 30s / 60s / 120s）的真实 MiMo 转写耗时，
    // 以此选定稳定吞吐的攒窗大小。
    let config = MimoConfig::new(key);
    let service = MimoAsrService::new(config);
    let format = AudioFormat::standard_speech();
    use std::io::Write;
    for secs in [5u64, 15, 30, 60, 120] {
        let bytes = secs as usize * 16_000 * 2;
        let slice = &pcm[..bytes.min(pcm.len())];
        let now = std::time::Instant::now();
        match service.transcribe_file(slice.to_vec(), format).await {
            Ok(result) => println!(
                "[{secs:>3}s] {:.1}s →  {text}",
                now.elapsed().as_secs_f32(),
                text = result.text.chars().take(60).collect::<String>()
            ),
            Err(e) => println!("[{secs:>3}s] {:.1}s →  ❌ {e}", now.elapsed().as_secs_f32()),
        }
        std::io::stdout().flush().unwrap();
    }
}

/// 从 `.secrets/API.md` 提取 `### <section>` 段的 `key`（按段名精确匹配）。
fn extract_section_key(markdown: &str, section: &str) -> String {
    let mut in_section = false;
    for raw_line in markdown.lines() {
        let line = raw_line.trim();
        if let Some(heading) = line.strip_prefix("###") {
            in_section = heading.trim().eq_ignore_ascii_case(section);
            continue;
        }
        if !in_section || line.is_empty() {
            continue;
        }
        if let Some((name, value)) = line.split_once(':') {
            if name.trim().eq_ignore_ascii_case("key")
                || name.trim().eq_ignore_ascii_case("api_key")
            {
                let v = value.trim().to_owned();
                if !v.is_empty() {
                    return v;
                }
            }
        }
    }
    panic!("`.secrets/API.md` 的 {section} 段缺少 key");
}
