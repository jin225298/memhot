package com.angelkang.memhot.capture

import android.content.Context
import android.os.SystemClock
import app.tauri.plugin.JSObject
import java.security.MessageDigest

object CaptureState {
    @Volatile var recording = false
        private set
    private val recordingClock = RecordingClock(SystemClock::elapsedRealtime)
    private fun prefs(context: Context) = context.getSharedPreferences("memhot-state", Context.MODE_PRIVATE)

    fun recordingStarted(at: Long) { recordingClock.start(at); recording = true }
    fun recordingStopped() { recordingClock.stop(); recording = false }

    fun set(context: Context, state: String, error: String? = null) {
        prefs(context).edit().putString("state", state).putString("error", error).commit()
    }

    @Synchronized fun uploaded(context: Context, owner: String) {
        val prefs = prefs(context)
        val key = "uploaded:$owner"
        prefs.edit().putInt(key, prefs.getInt(key, 0) + 1).remove("syncError:$owner").commit()
    }
    fun syncFailed(context: Context, owner: String, error: String) {
        prefs(context).edit().putString("syncError:$owner", error).commit()
    }
    fun syncError(context: Context, owner: String): String? = prefs(context).getString("syncError:$owner", null)
    fun clearSyncError(context: Context, owner: String) { prefs(context).edit().remove("syncError:$owner").commit() }

    private fun tokenHash(token: String): String = MessageDigest.getInstance("SHA-256")
        .digest(token.toByteArray()).joinToString("") { "%02x".format(it) }
    fun blockAuthentication(context: Context, token: String) {
        if (SecureConfig(context).read().token != token) return
        prefs(context).edit().putString("authBlockedFor", tokenHash(token)).commit()
        set(context, if (recording) "recording" else "paused", "登录已过期，请重新登录；录音已保留")
    }
    fun authenticationBlocked(context: Context, token: String): Boolean = prefs(context).getString("authBlockedFor", null) == tokenHash(token)
    fun clearAuthenticationError(context: Context) { prefs(context).edit().remove("authBlockedFor").remove("error").commit() }

    fun snapshot(context: Context): JSObject {
        val prefs = prefs(context)
        var state = prefs.getString("state", "idle") ?: "idle"
        var error = prefs.getString("error", null)
        val config = SecureConfig(context).read()
        if (error == null) error = syncError(context, config.owner())
        if (config.token.isNotEmpty() && authenticationBlocked(context, config.token)) {
            error = "登录已过期，请重新登录；录音已保留"
        }
        if (state == "recording" && !recording) {
            state = "paused"
            error = "系统中断了录音，请打开应用重新开始；已提交片段仍在队列中"
        }
        val queued = CaptureQueue(context).use { it.pending(config.owner()) + it.blocked(config.owner()) }
        val blocked = CaptureQueue(context).use { it.blocked(config.owner()) }
        return JSObject().put("supported", true).put("state", state)
            .put("elapsedMs", if (recording) recordingClock.elapsedMs() else 0L)
            .put("queuedCount", queued).put("uploadedCount", prefs.getInt("uploaded:${config.owner()}", 0))
            .put("blockedCount", blocked).put("asrMode", LocalModel.mode(context)).put("localModel", LocalModel.status(context))
            .put("noSpeechCount", SpeechBackoff.count(context, config.owner())).put("retryAfterMs", SpeechBackoff.delay(context, config.owner()))
            .put("notificationsAllowed", DigestSyncWorker.notificationsAllowed(context))
            .put("notificationGuide", "设置中开启日报提醒；Android省电和一加后台限制可能延迟通知，打开应用也可补收材料").apply {
                if (error != null) put("error", error)
            }
    }
}
