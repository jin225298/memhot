package com.angelkang.memhot.capture

import org.junit.Assert.*
import org.junit.Test

class UploadPolicyTest {
    @Test fun onlyDurablyCommittedTextAcknowledgesAudio() {
        assertTrue(UploadPolicy.accepted(202, "ready"))
        assertTrue(UploadPolicy.accepted(200, "no_speech"))
        assertFalse(UploadPolicy.accepted(202, "queued"))
        assertFalse(UploadPolicy.accepted(202, "awaiting_audio"))
        assertFalse(UploadPolicy.accepted(500, "ready"))
    }

    @Test fun quotaAndConfigurationErrorsExplainHowToResumeWithoutDuplicateConsent() {
        val quota = UploadPolicy.failure(402, "quota_exhausted")
        assertEquals(UploadRecovery.RETRY, quota.recovery)
        assertTrue(quota.message.contains("额度"))
        val model = UploadPolicy.failure(409, "model_unconfigured")
        assertEquals(UploadRecovery.RETRY, model.recovery)
        assertTrue(model.message.contains("模型"))
        assertTrue(UploadPolicy.failure(502, "busy").message.contains("繁忙"))
        assertTrue(UploadPolicy.failure(429, "rate_limited").message.contains("限流"))
    }

    @Test fun uncertainPaidCallsNeedConsentWhileDeviceNetworkErrorsCanReplayTheSameId() {
        for (code in listOf("outcome_unknown", "truncated", "invalid_response", "response_too_large", "network", "timeout")) {
            assertEquals(code, UploadRecovery.CONFIRM, UploadPolicy.failure(502, code).recovery)
        }
        assertEquals(UploadRecovery.RETRY, UploadPolicy.failure(0, null).recovery)
        assertEquals(UploadRecovery.RETRY, UploadPolicy.failure(409, "audio_processing").recovery)
    }

    @Test fun onlyApplicationAuthenticationErrorsRequestLogin() {
        assertEquals(UploadRecovery.AUTHENTICATE, UploadPolicy.failure(401, "unauthorized").recovery)
        assertEquals(UploadRecovery.AUTHENTICATE, UploadPolicy.failure(403, null).recovery)
        val provider = UploadPolicy.failure(502, "provider_auth")
        assertEquals(UploadRecovery.RETRY, provider.recovery)
        assertTrue(provider.message.contains("服务"))
    }
}
