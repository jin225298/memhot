package com.angelkang.memhot.updater

import android.app.DownloadManager
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.provider.Settings
import androidx.core.content.FileProvider
import app.tauri.plugin.JSObject
import java.io.File
import org.json.JSONObject

/** All methods run on the plugin's single bounded worker, including recovery and hashing. */
class UpdateEngine(private val context: Context) {
    private val manager = context.getSystemService(Context.DOWNLOAD_SERVICE) as DownloadManager
    private val prefs = context.getSharedPreferences("memhot.updater.v1", Context.MODE_PRIVATE)
    private val current = ApkVerifier.installed(context)
    private var release: UpdateRelease? = null
    private var downloadId = -1L
    private var phase = "idle"
    private var error: String? = null
    private var downloaded = 0L
    private var verifiedStamp: String? = null

    init {
        try {
            release = prefs.getString("release", null)?.let { ManifestCodec.parse(it, Build.VERSION.SDK_INT) }
            downloadId = prefs.getLong("downloadId", -1)
            phase = prefs.getString("phase", "idle") ?: "idle"
            error = prefs.getString("error", null)
            require(phase in setOf("idle", "available", "downloading", "ready", "permission", "installing", "failed"))
            if (release == null || release!!.versionCode <= current.versionCode) reset()
            else {
                if (phase == "installing") phase = "ready"
                refresh()
            }
        } catch (_: Exception) {
            // Untrusted/corrupt persisted data never turns into a URL or filesystem path.
            downloadId = -1
            release = null
            phase = "idle"
            error = null
            prefs.edit().clear().commit()
        }
    }

    private fun canInstall() = Build.VERSION.SDK_INT < 26 || context.packageManager.canRequestPackageInstalls()

    fun snapshot(): JSObject = JSObject().put("supported", true)
        .put("currentVersion", current.versionName).put("currentVersionCode", current.versionCode)
        .put("phase", phase).put("release", release?.let { ManifestCodec.json(it) } ?: JSONObject.NULL)
        .put("downloadedBytes", downloaded).put("totalBytes", release?.size ?: 0)
        .put("canInstall", canInstall()).put("error", error ?: JSONObject.NULL)

    fun getStatus(): JSObject { refresh(); return snapshot() }

    fun checkUpdate(): JSObject {
        refresh()
        if (phase == "downloading") return snapshot()
        val next = UpdateNetwork.fetchManifest()
        if (next.versionCode <= current.versionCode) {
            reset()
        } else if (next == release && phase in setOf("ready", "permission", "installing")) {
            error = null
        } else {
            clearDownload()
            release = next
            phase = "available"
            error = null
        }
        save()
        return snapshot()
    }

    fun downloadUpdate(): JSObject {
        refresh()
        if (phase in setOf("downloading", "ready", "permission", "installing")) return snapshot()
        val next = release ?: throw IllegalArgumentException("请先检查更新")
        next.validate(Build.VERSION.SDK_INT)
        require(next.versionCode > current.versionCode) { "已是最新版本" }
        UpdateNetwork.preflight(next)
        clearDownload()
        val destination = updateFile(next)
        val request = DownloadManager.Request(Uri.parse(next.url))
            .setTitle("Memhot ${next.version}")
            .setDescription("下载应用更新")
            .setMimeType("application/vnd.android.package-archive")
            .setNotificationVisibility(DownloadManager.Request.VISIBILITY_VISIBLE)
            .setDestinationInExternalFilesDir(context, "updates", destination.name)
            .setAllowedOverMetered(true)
            .setAllowedOverRoaming(false)
        downloadId = manager.enqueue(request)
        phase = "downloading"
        error = null
        try { save() } catch (failure: Exception) { clearDownload(); throw failure }
        return snapshot()
    }

    fun cancelDownload(): JSObject {
        clearDownload()
        phase = if (release != null) "available" else "idle"
        error = null
        save()
        return snapshot()
    }

    /** This is the only path that opens package settings or the system installer. */
    fun prepareInstall(): Intent {
        refresh()
        val next = release ?: throw IllegalArgumentException("请先下载更新包")
        require(phase in setOf("ready", "permission", "installing")) { "更新包尚未下载完成" }
        // Recheck every property immediately before granting read access to Android's installer.
        ApkVerifier.verify(context, updateFile(next), next)
        val intent = if (!canInstall()) {
            phase = "permission"
            Intent(Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES, Uri.parse("package:${context.packageName}"))
        } else {
            phase = "installing"
            val uri = FileProvider.getUriForFile(context, "${context.packageName}.memhot.updates", updateFile(next))
            Intent(Intent.ACTION_VIEW).setDataAndType(uri, "application/vnd.android.package-archive")
                .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
        }
        error = null
        save()
        return intent
    }

    fun returnedToApp() {
        if (phase == "installing") { phase = "ready"; save() }
        if (phase == "permission" && canInstall()) { phase = "ready"; save() }
    }

    fun fail(message: String): JSObject {
        phase = "failed"
        error = message
        try { save() } catch (_: Exception) { }
        return snapshot()
    }

    private fun refresh() {
        val next = release ?: return
        if (phase == "failed") return
        if (downloadId < 0) {
            if (phase in setOf("downloading", "ready", "permission", "installing")) {
                clearDownload()
                fail("下载记录丢失，请重新下载")
            }
            return
        }
        try {
            val cursor = manager.query(DownloadManager.Query().setFilterById(downloadId))
                ?: throw IllegalArgumentException("下载记录不可用，请重新下载")
            cursor.use {
                require(it.moveToFirst()) { "下载已被移除，请重新下载" }
                val status = it.getInt(it.getColumnIndexOrThrow(DownloadManager.COLUMN_STATUS))
                val bytes = it.getLong(it.getColumnIndexOrThrow(DownloadManager.COLUMN_BYTES_DOWNLOADED_SO_FAR))
                val total = it.getLong(it.getColumnIndexOrThrow(DownloadManager.COLUMN_TOTAL_SIZE_BYTES))
                require(bytes <= next.size && (total < 0 || total == next.size)) { "更新包大小异常，请重新下载" }
                downloaded = bytes.coerceIn(0, next.size)
                when (status) {
                    DownloadManager.STATUS_PENDING, DownloadManager.STATUS_RUNNING, DownloadManager.STATUS_PAUSED -> {
                        phase = "downloading"
                    }
                    DownloadManager.STATUS_SUCCESSFUL -> {
                        val file = updateFile(next)
                        val local = it.getString(it.getColumnIndexOrThrow(DownloadManager.COLUMN_LOCAL_URI))
                        require(local != null && Uri.parse(local).scheme == "file" &&
                            File(Uri.parse(local).path ?: "").canonicalFile == file.canonicalFile) { "更新包存储位置异常" }
                        val stamp = "${file.length()}:${file.lastModified()}"
                        if (verifiedStamp != stamp) {
                            ApkVerifier.verify(context, file, next)
                            verifiedStamp = stamp
                        }
                        downloaded = next.size
                        if (phase !in setOf("permission", "installing")) phase = "ready"
                        if (phase == "permission" && canInstall()) phase = "ready"
                        save()
                    }
                    else -> throw IllegalArgumentException("更新下载失败，请检查网络和存储空间后重试")
                }
            }
        } catch (failure: Exception) {
            clearDownload()
            fail(safeMessage(failure, "无法恢复更新下载，请重新下载"))
        }
    }

    private fun updateFile(next: UpdateRelease): File {
        next.validate(Build.VERSION.SDK_INT)
        val root = context.getExternalFilesDir("updates") ?: throw IllegalArgumentException("更新存储空间不可用")
        require((root.exists() || root.mkdirs()) && root.isDirectory) { "无法创建更新目录" }
        val file = File(root, "memhot-${next.version}-${next.versionCode}.apk")
        require(file.canonicalFile.parentFile == root.canonicalFile) { "更新文件路径异常" }
        return file
    }

    private fun clearDownload() {
        if (downloadId >= 0) manager.remove(downloadId)
        downloadId = -1
        release?.let { updateFile(it).delete() }
        downloaded = 0
        verifiedStamp = null
    }

    private fun reset() { clearDownload(); release = null; phase = "idle"; error = null; save() }

    private fun save() {
        check(prefs.edit().putString("release", release?.let { ManifestCodec.json(it).toString() })
            .putLong("downloadId", downloadId).putString("phase", phase).putString("error", error).commit()) {
            "无法保存更新状态，请检查存储空间"
        }
    }

    companion object {
        fun safeMessage(failure: Exception, fallback: String): String {
            // Only intentional localized errors cross the native boundary, never URLs/diagnostics.
            val message = failure.message ?: return fallback
            return if (message.any { it in '\u4e00'..'\u9fff' } && message.length <= 120) message else fallback
        }
    }
}
