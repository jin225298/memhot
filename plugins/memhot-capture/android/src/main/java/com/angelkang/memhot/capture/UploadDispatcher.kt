package com.angelkang.memhot.capture

import android.app.job.JobInfo
import android.app.job.JobParameters
import android.app.job.JobScheduler
import android.app.job.JobService
import android.content.ComponentName
import android.content.Context
import android.os.PersistableBundle
import java.net.HttpURLConnection
import java.net.URL
import java.security.MessageDigest
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale
import java.util.TimeZone
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicBoolean
import org.json.JSONObject

object UploadDispatcher {
    private val active = AtomicBoolean(false)
    private val worker = Executors.newSingleThreadExecutor()
    @Volatile private var activeConnection: HttpURLConnection? = null
    private const val JOB_ID = 17341

    fun clearPending(context: Context, done: (Int, String?) -> Unit) {
        val stopped = CaptureService.requestStopNow()
        context.stopService(android.content.Intent(context, CaptureService::class.java))
        activeConnection?.disconnect()
        context.getSystemService(JobScheduler::class.java).cancel(JOB_ID)
        // Wait for prior local uploader tasks and recording commits before clearing files.
        // Disconnect is best effort; a request already sent may have committed remotely.
        // The UI waits for this local fence before issuing the server-side deletion.
        worker.execute {
            try {
                stopped?.join(5000)
                check(stopped?.isAlive != true) { "录音尚未停止，请稍后重试" }
                val config = SecureConfig(context).read()
                check(config.userId.isNotEmpty()) { "当前账户信息缺失，请重新登录" }
                val count = CaptureQueue(context).use { it.clearOwner(config.owner()) }
                CaptureState.set(context, "idle")
                done(count, null)
            } catch (_: Exception) { done(0, "无法完全清理本地录音，请停止录音并检查存储空间后重试") }
        }
    }

    @Synchronized fun schedule(context: Context, delayMs: Long = 3000, finishedBatch: Boolean = false) {
        val offline = try {
            val owner = SecureConfig(context).read().owner()
            CaptureQueue(context).use { it.needsLocalDecode(owner) }
        } catch (_: Exception) { false }
        val network = if (offline) JobInfo.NETWORK_TYPE_NONE else JobInfo.NETWORK_TYPE_ANY
        val scheduler = context.getSystemService(JobScheduler::class.java)
        val pending = if (finishedBatch) null else scheduler.getPendingJob(JOB_ID)
        val deadline = pending?.extras?.getLong("uploadDueAt", 0)?.takeIf { it > 0 }
        val now = System.currentTimeMillis()
        val delay = UploadSchedule.delay(now, delayMs, deadline, pending?.networkType == network) ?: return
        val extras = PersistableBundle().apply { putLong("uploadDueAt", now + delay) }
        val job = JobInfo.Builder(JOB_ID, ComponentName(context, UploadJobService::class.java))
            .setRequiredNetworkType(network).setPersisted(true).setExtras(extras)
            .setMinimumLatency(delay).setBackoffCriteria(30_000, JobInfo.BACKOFF_POLICY_EXPONENTIAL)
            .build()
        scheduler.schedule(job)
    }

    fun upload(context: Context, cancelled: () -> Boolean = { false }, done: () -> Unit = {}) {
        if (!active.compareAndSet(false, true)) { done(); return }
        worker.execute {
            var recognizer: LocalRecognizer? = null
            try {
                CaptureQueue(context).use { queue ->
                    queue.recover()
                    // A bounded batch also bounds the lifetime of a background job.
                    repeat(8) {
                        if (cancelled()) return@use
                        val config = SecureConfig(context).read()
                        if (config.token.isEmpty()) return@use
                        if (CaptureState.authenticationBlocked(context, config.token)) return@use
                        if (SpeechBackoff.delay(context, config.owner()) > 0) return@use
                        var item = queue.next(config.owner()) ?: return@use
                        try {
                            if (item.mode == "local" && item.text == null) {
                                check(LocalModel.ready(context)) { "离线模型不可用，录音保留在本机；请下载模型后继续" }
                                verify(item)
                                if (recognizer == null) recognizer = LocalRecognizer(context)
                                val text = recognizer!!.transcribe(item.file.readBytes())
                                queue.transcribed(item, text)
                                item = item.copy(text = text)
                                if (text.isBlank()) {
                                    queue.acknowledged(item)
                                    SpeechBackoff.result(context, config.owner(), true)
                                    return@repeat
                                }
                            }
                            val response = send(config, item, queue.retryKey(item))
                            val code = response.first
                            if (UploadPolicy.accepted(code, response.second.optString("status"))) {
                                queue.acknowledged(item)
                                CaptureState.uploaded(context, config.owner())
                                SpeechBackoff.result(context, config.owner(), response.second.optString("status") == "no_speech")
                            } else {
                                val failure = UploadPolicy.failure(code, response.second.optJSONObject("error")?.optString("code"))
                                if (failure.recovery == UploadRecovery.CONFIRM) queue.block(item) else queue.retry(item)
                                CaptureState.syncFailed(context, config.owner(), failure.message)
                                if (failure.recovery == UploadRecovery.AUTHENTICATE) {
                                    CaptureState.blockAuthentication(context, config.token)
                                }
                                return@use
                            }
                        } catch (error: Throwable) {
                            queue.retry(item)
                            if (error !is IllegalStateException) CaptureState.syncFailed(context, config.owner(), UploadPolicy.failure(0, null).message)
                            // Local inference remains useful offline: finish other bounded raw items,
                            // keeping their durable text for network recovery without retaining audio.
                            if (item.mode == "local" && item.text != null) return@repeat
                            if (item.mode == "local") CaptureState.set(context,
                                if (CaptureState.recording) "recording" else "error", "离线转写暂不可用；录音保留在本机，请检查模型及可用空间")
                            else if (error is IllegalStateException) CaptureState.set(context,
                                if (CaptureState.recording) "recording" else "error", "本地录音片段完整性异常，已保留，请检查设备存储空间")
                            return@use
                        }
                    }
                }
            } catch (_: Exception) {
                CaptureState.set(context, if (CaptureState.recording) "recording" else "error", "本地上传队列无法读取，请检查存储空间")
            } finally {
                try { recognizer?.close() } catch (_: Throwable) { /* Release the queue fence even if JNI cleanup fails. */ }
                active.set(false)
                // Finish the old JobScheduler execution before scheduling its successor.
                done()
                try {
                    val config = SecureConfig(context).read()
                    if (config.token.isNotEmpty() && !CaptureState.authenticationBlocked(context, config.token)) {
                        CaptureQueue(context).use {
                            if (it.pending(config.owner()) > 0) schedule(context, maxOf(it.nextDelay(config.owner()), SpeechBackoff.delay(context, config.owner())), finishedBatch = true)
                        }
                    }
                } catch (_: Exception) { /* Preserve queue files and retry on next user launch. */ }
            }
        }
    }

    private fun verify(item: QueuedAudio) {
        check(item.file.exists()) { "Queued audio file unavailable" }
        check(item.file.length() in 45..960_044) { "Invalid audio length" }
        val actualHash = MessageDigest.getInstance("SHA-256").digest(item.file.readBytes())
            .joinToString("") { "%02x".format(it) }
        check(actualHash == item.sha256) { "Queued audio integrity check failed" }
    }
    private fun send(config: DeviceConfig, item: QueuedAudio, retryKey: String?): Pair<Int, JSONObject> {
        if (item.mode == "cloud") verify(item)
        val local = item.mode == "local"
        val endpoint = if (local) "/api/v1/audio/transcript" else "/api/v1/audio"
        val connection = URL("${config.baseUrl}$endpoint").openConnection() as HttpURLConnection
        activeConnection = connection
        try {
            connection.requestMethod = "POST"
            connection.instanceFollowRedirects = false // Never forward credentials to a redirect target.
            connection.connectTimeout = 15_000
            connection.readTimeout = 130_000
            connection.doOutput = true
            connection.setRequestProperty("Authorization", "Bearer ${config.token}")
            connection.setRequestProperty("Content-Type", if (local) "application/json" else "audio/wav")
            connection.setRequestProperty("Idempotency-Key", item.id)
            val date = SimpleDateFormat("yyyy-MM-dd'T'HH:mm:ss.SSS'Z'", Locale.US).apply {
                timeZone = TimeZone.getTimeZone("UTC")
            }.format(Date(item.capturedAt))
            connection.setRequestProperty("X-Captured-At", date)
            if (retryKey != null) {
                connection.setRequestProperty("X-Audio-Retry-Key", retryKey)
                connection.setRequestProperty("X-Accept-Duplicate-Charge", "true")
            }
            if (local) {
                val body = JSONObject().put("clientId", item.id).put("text", item.text ?: error("transcript"))
                    .put("capturedAt", date).put("durationMs", item.durationMs).put("modelId", LocalModel.ID)
                    .put("modelVersion", LocalModel.VERSION).put("audioHash", item.sha256).toString().toByteArray()
                connection.setFixedLengthStreamingMode(body.size)
                connection.outputStream.use { it.write(body) }
            } else {
                connection.setFixedLengthStreamingMode(item.file.length())
                connection.outputStream.use { out -> item.file.inputStream().use { it.copyTo(out, 16_384) } }
            }
            val code = connection.responseCode
            val input = if (code in 200..299) connection.inputStream else connection.errorStream
            val body = input?.use { stream ->
                val buffer = java.io.ByteArrayOutputStream()
                val bytes = ByteArray(4096)
                while (true) { val n = stream.read(bytes); if (n < 0) break; check(buffer.size() + n <= 65536); buffer.write(bytes, 0, n) }
                buffer.toString("UTF-8")
            } ?: "{}"
            return code to try { JSONObject(body) } catch (_: Exception) { JSONObject() }
        } finally { connection.disconnect(); activeConnection = null }
    }
}

class UploadJobService : JobService() {
    @Volatile private var cancelled = false
    override fun onStartJob(params: JobParameters): Boolean {
        cancelled = false
        UploadDispatcher.upload(applicationContext, { cancelled }) {
            if (!cancelled) jobFinished(params, false)
        }
        return true
    }
    override fun onStopJob(params: JobParameters): Boolean { cancelled = true; return true }
}
