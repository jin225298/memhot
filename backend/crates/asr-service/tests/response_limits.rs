use asr_service::{
    AsrService, AudioFormat, MimoAsrService, MimoConfig, Qwen3AsrConfig, Qwen3AsrService,
};
use std::{
    io::{Read, Write},
    net::TcpListener,
    time::Duration,
};

fn mock_provider(response: Vec<u8>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let attempts = if response.starts_with(b"HTTP/1.1 429") {
        4
    } else {
        1
    };
    std::thread::spawn(move || {
        for _ in 0..attempts {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut request = Vec::new();
            let mut buffer = [0; 4096];
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                let count = socket.read(&mut buffer).unwrap();
                if count == 0 {
                    return;
                }
                request.extend_from_slice(&buffer[..count]);
            }
            let end = request.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
            let headers = String::from_utf8_lossy(&request[..end]).to_ascii_lowercase();
            let length: usize = headers
                .lines()
                .find_map(|line| line.strip_prefix("content-length: "))
                .unwrap_or("0")
                .parse()
                .unwrap();
            while request.len() < end + length {
                let count = socket.read(&mut buffer).unwrap();
                if count == 0 {
                    return;
                }
                request.extend_from_slice(&buffer[..count]);
            }
            // Early response rejection intentionally closes the connection before all bytes are written.
            let _ = socket.write_all(&response);
        }
    });
    format!("http://{address}/v1")
}

async fn transcribe(
    response: Vec<u8>,
    mimo: bool,
) -> asr_service::Result<asr_service::TranscriptionResult> {
    let endpoint = mock_provider(response);
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let service: Box<dyn AsrService> = if mimo {
        Box::new(MimoAsrService::with_client(
            MimoConfig::new("test".into()).with_base_url(endpoint),
            client,
        ))
    } else {
        Box::new(Qwen3AsrService::with_client(
            Qwen3AsrConfig::new("test".into()).with_base_url(endpoint),
            client,
        ))
    };
    service
        .transcribe_file(vec![0; 320], AudioFormat::standard_speech())
        .await
}

#[tokio::test]
async fn both_adapters_reject_oversized_advertised_response_without_reading_body() {
    for mimo in [false, true] {
        let error = transcribe(
            b"HTTP/1.1 200 OK\r\nContent-Length: 2097153\r\n\r\n".to_vec(),
            mimo,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("2 MiB limit"));
    }
}

#[tokio::test]
async fn both_adapters_bound_chunked_responses() {
    let mut response = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n200001\r\n".to_vec();
    response.extend(vec![b' '; 2 * 1024 * 1024 + 1]);
    response.extend_from_slice(b"\r\n0\r\n\r\n");
    for mimo in [false, true] {
        let error = transcribe(response.clone(), mimo).await.unwrap_err();
        assert!(error.to_string().contains("2 MiB limit"));
    }
}

#[tokio::test]
async fn provider_errors_never_include_response_body() {
    for mimo in [false, true] {
        let response =
            b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 18\r\n\r\nprivate-transcript".to_vec();
        let error = transcribe(response, mimo).await.unwrap_err();
        assert!(matches!(
            error,
            asr_service::AsrError::AuthenticationError(_)
        ));
        assert!(!error.to_string().contains("private-transcript"));
    }
}

#[tokio::test]
async fn both_adapters_preserve_http_transience_and_reject_incomplete_success() {
    for mimo in [false, true] {
        for (status, expected) in [
            (503, "unavailable"),
            (500, "unavailable"),
            (429, "rate_limit"),
        ] {
            let wire = format!("HTTP/1.1 {status} Temporary Failure\r\nContent-Length: 0\r\n\r\n")
                .into_bytes();
            let error = transcribe(wire, mimo).await.unwrap_err();
            assert!(
                if status == 429 {
                    matches!(error, asr_service::AsrError::RateLimitExceeded)
                } else {
                    matches!(error, asr_service::AsrError::ServiceUnavailable)
                },
                "{expected}: {error:?}"
            );
        }
        for finish in ["length", "content_filter", "stop"] {
            let body=serde_json::json!({"choices":[{"message":{"content":"partial spoken text"},"finish_reason":finish}]}).to_string();
            let response=format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",body.len()).into_bytes();
            let result = transcribe(response, mimo).await;
            match finish {
                "length" => assert!(matches!(
                    result,
                    Err(asr_service::AsrError::TruncatedResponse)
                )),
                "content_filter" => assert!(matches!(
                    result,
                    Err(asr_service::AsrError::InvalidResponse(_))
                )),
                _ => assert_eq!(result.unwrap().text, "partial spoken text"),
            }
        }
    }
}
