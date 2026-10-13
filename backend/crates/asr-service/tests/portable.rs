use asr_service::{
    AsrConfig, AsrError, AsrService, AudioFormat, MimoAsrService, MimoConfig, Qwen3AsrConfig,
    Qwen3AsrService,
};

#[test]
fn mimo_endpoint_and_model_can_be_configured() {
    let config = MimoConfig::new("private-test-key".into())
        .with_base_url("https://gateway.invalid/v1/")
        .with_model("custom-asr");
    assert_eq!(config.endpoint(), "https://gateway.invalid/v1/");
    assert_eq!(config.model(), "custom-asr");
}

#[test]
fn debugging_qwen_config_never_prints_credentials() {
    let config = Qwen3AsrConfig::new("private-test-key".into());
    assert!(!format!("{config:?}").contains("private-test-key"));
}

#[tokio::test]
async fn invalid_pcm_fails_before_outbound_network() {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_millis(1))
        .build()
        .unwrap();
    let qwen = Qwen3AsrService::with_client(
        Qwen3AsrConfig::new("test".into()).with_base_url("https://invalid.invalid"),
        client.clone(),
    );
    let mimo = MimoAsrService::with_client(
        MimoConfig::new("test".into()).with_base_url("https://invalid.invalid"),
        client,
    );
    for service in [&qwen as &dyn AsrService, &mimo as &dyn AsrService] {
        let error = service
            .transcribe_file(vec![0], AudioFormat::standard_speech())
            .await
            .unwrap_err();
        assert!(matches!(error, AsrError::UnsupportedFormat(_)));
    }
}
