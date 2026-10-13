use asr_service::{FiletransPoll, FiletransSubmission, QwenFiletransConfig, QwenFiletransService};
use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let markdown = std::fs::read_to_string(".secrets/API.md")?;
    let config = QwenFiletransConfig::from_api_markdown(&markdown)?;
    let service = QwenFiletransService::new(config);
    let path = std::env::var_os("QWEN_FILETRANS_AUDIO_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(".target-qwen/welcome.mp3"));
    let submission = service.submit_wav(&path).await?;
    let result = match submission {
        FiletransSubmission::Completed(result) => result,
        FiletransSubmission::Pending { task_id, poll_url } => {
            println!("task={task_id}");
            let mut result = None;
            for _ in 0..12 {
                eprintln!("polling task");
                match service.poll_task(&task_id, poll_url.as_deref()).await? {
                    FiletransPoll::Completed(value) => {
                        result = Some(value);
                        break;
                    }
                    FiletransPoll::Failed(message) => return Err(message.into()),
                    FiletransPoll::Processing => {
                        tokio::time::sleep(std::time::Duration::from_secs(2)).await
                    }
                }
            }
            result.ok_or("poll timeout")?
        }
    };
    println!("segments={}", result.segments.len());
    for segment in result.segments {
        println!(
            "{} {}-{} {:?}",
            segment.text, segment.start_ms, segment.end_ms, segment.speaker
        );
    }
    Ok(())
}
