# Audio synchronization repair

The existing Android queue → cloud relay → Memocat ASR provider → durable text pipeline remains authoritative. Successful acknowledgement deletes device audio; offline, rejected and uncertain requests retain the device copy. No playback or cloud audio archive is introduced. The application's raw-audio handling uses request memory; this statement does not cover gateway buffering. Nginx's exact `/api/v1/audio` route must disable `proxy_request_buffering` so it streams to authenticated admission without first buffering the complete WAV to a temporary file.

## Minimal changes

1. Classify upload results in a small Kotlin policy. Display actionable messages for quota, configuration, authentication, capacity and network failures. Uncertain paid results remain blocked until explicit retry; ordinary retries reuse their original receipt key. Keep synchronization errors separate from recording errors and scoped to the current queue owner.
2. Preserve the earliest Android upload deadline instead of replacing it on every speech segment. New segments request a three-second first attempt, while existing retry/backoff deadlines remain authoritative. Retain single-threaded upload and speech gating.
3. Authenticate and acquire a non-waiting ASR admission permit before extracting the WAV body. Limit admitted ASR requests to three globally and one per user, independently of text-model permits. Admission failure returns busy before reading or retaining request audio and before reserving credits.
4. Give only ASR transport a 90-second timeout, bounded by the existing 100-second relay deadline, 120-second proxy wait and 130-second Android wait. DNS and retry work remain inside the relay deadline. Unknown outcomes keep their durable receipts and never automatically repeat a paid call.
5. Add the actual awaiting_audio source state to the frontend's Chinese labels and type.

Admission bounds the application's simultaneous accepted WAV bodies to three, each still subject to the existing 8 MiB HTTP body limit. Transcoding to the provider's WAV/base64 request uses additional bounded temporary memory. The device captures substantially smaller speech segments. This repair does not claim a full production memory peak measurement or alter provider rate limits.

## Verification

- Kotlin policy tests cover ordinary/uncertain/authentication failures and earliest-deadline scheduling under repeated speech.
- Android instrumentation covers owner-scoped synchronization errors and successful acknowledgement retaining only text.
- Rust HTTP tests prove saturated admission and unauthorized requests do not poll the request body; text saturation cannot block ASR admission; per-user and global ASR limits release correctly.
- Existing audio-ingest and billing tests continue to verify tenant isolation, idempotency, unknown-result protection, no audio files and durable text acknowledgements.
- No production provider calls or deployment are required.

## Local evidence

- Final combined server verification passes 128 tests across library, audio admission/ingest, API, state and account/billing suites; one pre-existing test remains ignored. This final run includes the concurrently completed AIHOT tests and the final typed ASR error mappings.
- The initial admission regressions failed against the previous implementation: two rejected requests consumed their WAV bodies, and a completed audio receipt could not replay within 250 ms while text slots were occupied. All four admission tests pass with the new extractor.
- All 18 capture JVM tests pass, including nine new result-policy/scheduling tests.
- Three Android 16 emulator instrumentation tests pass: actual JobScheduler deadline preservation, account-scoped synchronization error recovery, and retaining a cloud WAV until acknowledgement then removing it durably.
- Two real local TCP tests with a controlled Tokio clock verify that ASR accepts a response after 61 seconds and still times out without a response after 91 seconds. No production provider is called.
- The reused ASR crate passes 36 local library/portable/response-limit tests; one pre-existing test remains ignored.
- Frontend TypeScript checking and the nine native/updater boundary tests pass. Existing Gradle deprecation warnings and the shared Rust fixture's unused `note` helper remain unrelated to this repair.

Logs are local under `/Users/kang/Library/Caches/memhot-tools/audio-sync-{server-tests,provider-tests,instrumentation}.log`. Production load, real speech latency, handset power management and final release deployment are separate acceptance steps.
