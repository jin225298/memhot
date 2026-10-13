package com.angelkang.memhot.capture

import android.Manifest
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.pm.PackageManager
import android.os.Build
import androidx.core.app.NotificationManagerCompat
import androidx.work.Constraints
import androidx.work.ExistingPeriodicWorkPolicy
import androidx.work.NetworkType
import androidx.work.PeriodicWorkRequest
import androidx.work.WorkManager
import androidx.work.Worker
import androidx.work.WorkerParameters
import org.json.JSONObject
import java.net.HttpURLConnection
import java.net.URL
import java.util.concurrent.TimeUnit

/** No GMS, microphone or WebView is needed. OEM/Doze may delay this work. */
class DigestSyncWorker(context: Context, params: WorkerParameters) : Worker(context, params) {
    override fun doWork(): Result {
        try {
            val config = SecureConfig(applicationContext).read()
            if (config.token.isEmpty() || !notificationsAllowed(applicationContext)) return Result.success()
            if (CaptureState.authenticationBlocked(applicationContext, config.token)) return Result.success()
            val connection = URL("${config.baseUrl}/api/v1/delivery").openConnection() as HttpURLConnection
            val json: JSONObject
            try {
                connection.instanceFollowRedirects = false
                connection.connectTimeout = 15_000; connection.readTimeout = 20_000
                connection.setRequestProperty("Authorization", "Bearer ${config.token}")
                val code = connection.responseCode
                if (code == 401 || code == 403) {
                    CaptureState.blockAuthentication(applicationContext, config.token)
                    return Result.success()
                }
                if (code != 200) return if (runAttemptCount < 3) Result.retry() else Result.failure()
                val bytes = connection.inputStream.use { it.readBytesLimited(1_048_576) }
                json = JSONObject(String(bytes, Charsets.UTF_8))
            } finally { connection.disconnect() }
            // Configuration can change while a network request is pending.
            val current = SecureConfig(applicationContext).read()
            if (current.owner() != config.owner() || current.token.isEmpty()) return Result.success()
            val reports = json.optJSONArray("reports") ?: return Result.success()
            val prefs = applicationContext.getSharedPreferences("memhot-delivery", Context.MODE_PRIVATE)
            val key = config.owner()
            val seen = prefs.getStringSet(key, null)
            val ids = (0 until minOf(reports.length(), 100)).mapNotNull { reports.optJSONObject(it)?.optString("id")?.takeIf { id -> id.isNotEmpty() } }
            // First sync records a baseline instead of sending historical reports as new.
            if (seen != null) {
                ReportDelivery.unseen(ids, seen).take(3).forEach { id -> notifyReport(id) }
            }
            prefs.edit().putStringSet(key, (ids + (seen ?: emptySet())).take(200).toSet()).commit()
            return Result.success()
        } catch (_: Exception) { return if (runAttemptCount < 3) Result.retry() else Result.failure() }
    }

    private fun notifyReport(id: String) {
        if (!notificationsAllowed(applicationContext)) return
        val manager = applicationContext.getSystemService(NotificationManager::class.java)
        if (Build.VERSION.SDK_INT >= 26) manager.createNotificationChannel(NotificationChannel(CHANNEL, "Memhot 新研究材料", NotificationManager.IMPORTANCE_DEFAULT))
        val launch = applicationContext.packageManager.getLaunchIntentForPackage(applicationContext.packageName) ?: return
        launch.putExtra("memhotLaunchTarget", "today")
        launch.addFlags(android.content.Intent.FLAG_ACTIVITY_CLEAR_TOP or android.content.Intent.FLAG_ACTIVITY_SINGLE_TOP)
        val pending = PendingIntent.getActivity(applicationContext, id.hashCode(), launch, PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE)
        val builder = if (Build.VERSION.SDK_INT >= 26) Notification.Builder(applicationContext, CHANNEL) else Notification.Builder(applicationContext)
        manager.notify(id, 17343, builder.setContentTitle("Memhot 有新的思考材料")
            .setContentText("研究材料已就绪，打开后继续思考") // Keep private report titles off the lock screen.
            .setSmallIcon(android.R.drawable.ic_menu_info_details).setContentIntent(pending).setAutoCancel(true).build())
    }

    companion object {
        private const val UNIQUE = "memhot-digest-sync"
        private const val CHANNEL = "memhot-digests"
        fun configure(context: Context, connected: Boolean) {
            val work = WorkManager.getInstance(context)
            if (!connected) { work.cancelUniqueWork(UNIQUE); return }
            val request = PeriodicWorkRequest.Builder(DigestSyncWorker::class.java, 30, TimeUnit.MINUTES)
                .setConstraints(Constraints.Builder().setRequiredNetworkType(NetworkType.CONNECTED).build()).build()
            work.enqueueUniquePeriodicWork(UNIQUE, ExistingPeriodicWorkPolicy.UPDATE, request)
        }
        fun notificationsAllowed(context: Context): Boolean =
            (Build.VERSION.SDK_INT < 33 || context.checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED) &&
                NotificationManagerCompat.from(context).areNotificationsEnabled()
    }
}

private fun java.io.InputStream.readBytesLimited(max: Int): ByteArray {
    val out = java.io.ByteArrayOutputStream()
    val buffer = ByteArray(8192)
    while (true) {
        val count = read(buffer)
        if (count < 0) break
        check(out.size() + count <= max) { "Response exceeds limit" }
        out.write(buffer, 0, count)
    }
    return out.toByteArray()
}
