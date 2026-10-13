package com.angelkang.memhot.capture

enum class UploadRecovery { RETRY, AUTHENTICATE, CONFIRM }
data class UploadFailure(val recovery: UploadRecovery, val message: String)

/** App errors never expose provider bodies, and only a durable transcript acknowledges audio. */
object UploadPolicy {
    fun accepted(http: Int, status: String) = http in 200..299 && status in listOf("ready", "no_speech")

    fun failure(http: Int, code: String?): UploadFailure {
        if (http == 401 || http == 403) return UploadFailure(UploadRecovery.AUTHENTICATE,
            "登录已过期，请重新登录；录音保留在本机")
        if (code in listOf("outcome_unknown", "truncated", "invalid_response", "response_too_large", "network", "timeout")) {
            return UploadFailure(UploadRecovery.CONFIRM,
                "上次转写结果不明，录音保留在本机；请检查后明确重试，可能再次使用额度")
        }
        if (code == "idempotency_conflict") return UploadFailure(UploadRecovery.CONFIRM,
            "录音片段校验冲突，原音已保留；请检查账户和设备后重新处理")
        val reason = when (code) {
            "quota_exhausted" -> "转写额度不足，请到账号中补充额度后重试"
            "model_unconfigured" -> "转写模型尚未配置，请检查服务配置后重试"
            "provider_auth", "asr_failed", "provider_rejected" -> "转写服务配置或凭据异常，请检查服务后重试"
            "busy" -> "转写服务繁忙，将自动重试"
            "rate_limited" -> "转写服务限流，将稍后自动重试"
            "audio_processing" -> "该片段仍在转写，将稍后检查结果"
            "invalid_input", "payload_too_large" -> "录音片段未被服务接受，请检查后重试"
            else -> if (http == 0) "网络暂不可用，联网后将自动同步" else "语音同步暂未完成，将自动重试"
        }
        return UploadFailure(UploadRecovery.RETRY, "$reason；录音保留在本机")
    }
}
