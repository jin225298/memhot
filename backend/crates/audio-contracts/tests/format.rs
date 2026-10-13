use audio_contracts::AudioFormat;

#[test]
fn speech_pcm_rejects_partial_samples_and_invalid_format() {
    let format = AudioFormat::standard_speech();
    assert_eq!(format.duration_ms(32_000), Some(1000));
    assert!(format.validate_pcm16(32_000).is_ok());
    assert!(format.validate_pcm16(31_999).is_err());
    assert!(format.validate_pcm16(0).is_err());
    let invalid = AudioFormat {
        sample_rate: 0,
        ..format
    };
    assert_eq!(invalid.duration_ms(32_000), None);
    assert!(invalid.validate_pcm16(32_000).is_err());
}

#[test]
fn stereo_duration_uses_frame_size() {
    assert_eq!(AudioFormat::high_quality().duration_ms(192_000), Some(1000));
}
