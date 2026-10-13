# Speech components

Memhot uses these upstream projects. Model weights are downloaded on demand; they are not included in the source tree or APK.

| Component | Version | License and attribution |
| --- | --- | --- |
| sherpa-onnx Android runtime | 1.13.8 | Apache-2.0, copyright k2-fsa and contributors; https://github.com/k2-fsa/sherpa-onnx/blob/v1.13.8/LICENSE |
| Apache Commons Compress | 1.28.0 | Apache-2.0; https://commons.apache.org/proper/commons-compress/ |
| Silero VAD | pinned SHA-256 in Android build | MIT, copyright Silero Team; https://github.com/snakers4/silero-vad/blob/master/LICENSE |
| SenseVoiceSmall int8 model | 2024-07-17; converted by csukuangfj | FunASR Model Open Source License 1.1 (model weights); SenseVoice code MIT. Developed by FunAudioLLM / Alibaba. https://github.com/FunAudioLLM/SenseVoice#license |

The downloadable model is SenseVoiceSmall, not an independently trained Memhot model. Upstream conversion: https://huggingface.co/csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17/tree/2365baeacb507f821a0c8120fcee3d484dba7a07 . The GitHub archive download is 163,002,883 bytes; the installed files total 239,549,735 bytes. The manifest pins the official release archive SHA-256 and both extracted file sizes and SHA-256 digests; changing a model requires an explicit reviewed manifest change. SenseVoiceSmall's model license is distinct from the Apache license of its runtime.

Runtime/VAD artifacts are fetched during Android build with SHA-256 verification. Offline ASR assets are fetched only when the user requests installation and remain in Android `noBackupFilesDir`. Uninstalling the model does not switch queued recordings to cloud ASR.

## Verification

Run JVM and synthetic-device checks with `:tauri-plugin-memhot-capture:testDebugUnitTest` and `:tauri-plugin-memhot-capture:connectedDebugAndroidTest`. The optional full model download/install/decode/remove integration check additionally uses `-Pandroid.testInstrumentationRunnerArguments.downloadModel=true -Pandroid.testInstrumentationRunnerArguments.class=com.angelkang.memhot.capture.ModelInstallTest`; it requires public GitHub connectivity and roughly 280 MB free storage. It never requests microphone input.

The pinned arm64-v8a and x86_64 runtime libraries have 16 KiB ELF PT_LOAD alignment. Device memory consumption, sustained battery drain and speech accuracy still require real-device measurements; emulator loading is not a battery benchmark.
