package com.angelkang.memhot.capture

import android.content.Context
import app.tauri.plugin.JSObject
import java.io.File
import java.io.FileOutputStream
import java.net.HttpURLConnection
import java.net.URL
import java.security.MessageDigest
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicBoolean

/** Public upstream model artifacts; no account credentials ever enter download requests. */
object LocalModel {
    const val ID = "sensevoice-small-int8"
    const val VERSION = "2024-07-17"
    const val LICENSE = "https://github.com/FunAudioLLM/SenseVoice#license"
    private const val ARCHIVE = "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17.tar.bz2"
    private const val ARCHIVE_BYTES = 163002883L
    private const val ARCHIVE_HASH = "7d1efa2138a65b0b488df37f8b89e3d91a60676e416f515b952358d83dfd347e"
    private data class Asset(val name: String, val bytes: Long, val hash: String)
    private val assets = listOf(
        Asset("model.int8.onnx", 239233841, "c71f0ce00bec95b07744e116345e33d8cbbe08cef896382cf907bf4b51a2cd51"),
        Asset("tokens.txt", 315894, "f449eb28dc567533d7fa59be34e2abca8784f771850c78a47fb731a31429a1dc"),
    )
    private val executor = Executors.newSingleThreadExecutor()
    private val downloading = AtomicBoolean(false)
    private val cancelled = AtomicBoolean(false)
    @Volatile private var connection: HttpURLConnection? = null
    @Volatile private var downloaded = 0L
    @Volatile private var message: String? = null
    @Volatile private var errorCode: String? = null
    @Volatile var inUse = false
        private set

    fun directory(context: Context) = File(context.noBackupFilesDir, "speech-model/$ID-$VERSION")
    private fun prefs(context: Context) = context.getSharedPreferences("memhot-speech", Context.MODE_PRIVATE)
    fun mode(context: Context): String = prefs(context).getString("mode", "cloud") ?: "cloud"
    fun ready(context: Context): Boolean = directory(context).let { dir ->
        File(dir, "verified").isFile && assets.all { File(dir, it.name).length() == it.bytes }
    }
    fun setMode(context: Context, mode: String) {
        require(mode == "cloud" || mode == "local") { "转写模式无效" }
        check(!CaptureState.recording) { "请先停止录音再切换转写方式" }
        check(mode != "local" || ready(context)) { "请先下载并校验离线语音模型" }
        check(prefs(context).edit().putString("mode", mode).commit()) { "无法保存转写方式" }
    }
    fun status(context: Context): JSObject = JSObject().put("id", ID).put("version", VERSION)
        .put("state", if (downloading.get()) "downloading" else if (ready(context)) "ready" else if (message != null) "error" else "missing")
        .put("downloadedBytes", if (ready(context)) ARCHIVE_BYTES else downloaded)
        .put("totalBytes", ARCHIVE_BYTES).put("downloadBytes", ARCHIVE_BYTES).put("installedBytes", assets.sumOf { it.bytes })
        .put("licenseUrl", LICENSE).put("licenseName", "FunASR Model Open Source License 1.1")
        .put("runtimeLicense", "sherpa-onnx Apache-2.0 · Silero VAD MIT")
        .apply { message?.let { put("error", it) } }
        .apply { errorCode?.let { put("errorCode", it) } }

    fun download(context: Context) {
        if (ready(context) || !downloading.compareAndSet(false, true)) return
        cancelled.set(false); downloaded = 0; message = null; errorCode = null
        val app = context.applicationContext
        executor.execute {
            val dir = directory(app)
            try {
                check(dir.exists() || dir.mkdirs())
                check(dir.usableSpace > assets.sumOf { it.bytes } + 32L * 1024 * 1024) { "空间不足" }
                File(dir, "verified").delete()
                val remote = connect(ARCHIVE)
                val md = MessageDigest.getInstance("SHA-256")
                val progress = object : java.io.FilterInputStream(remote.inputStream) {
                    override fun read(): Int {
                        val value = `in`.read()
                        if (value >= 0) progress(1)
                        return value
                    }
                    override fun read(bytes: ByteArray, offset: Int, length: Int): Int {
                        val count = `in`.read(bytes, offset, length)
                        if (count > 0) progress(count)
                        return count
                    }
                    private fun progress(count: Int) {
                        check(!cancelled.get()) { "cancelled" }
                        downloaded += count
                        check(downloaded <= ARCHIVE_BYTES) { "size" }
                    }
                }
                val seen = mutableSetOf<String>()
                java.io.BufferedInputStream(java.security.DigestInputStream(progress, md), 65536).use { input ->
                    boundedDecoded(org.apache.commons.compress.compressors.bzip2.BZip2CompressorInputStream(input)).use { decoded ->
                        org.apache.commons.compress.archivers.tar.TarArchiveInputStream(decoded).use { tar ->
                            while (true) {
                                val entry = tar.nextEntry ?: break
                                val asset = assets.find { entry.name == "sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17/" + it.name } ?: continue
                                check(entry.isFile && !entry.isSymbolicLink && !entry.isLink && entry.size == asset.bytes && seen.add(asset.name)) { "size" }
                                val part = File(dir, asset.name + ".part")
                                var count = 0L
                                FileOutputStream(part).use { output ->
                                    val bytes = ByteArray(65536)
                                    while (true) {
                                        check(!cancelled.get()) { "cancelled" }
                                        val read = tar.read(bytes)
                                        if (read < 0) break
                                        count += read; check(count <= asset.bytes) { "size" }
                                        output.write(bytes, 0, read)
                                    }
                                    output.fd.sync()
                                }
                                check(count == asset.bytes && digest(part) == asset.hash) { "checksum" }
                            }
                            // Tar ends before the compression stream trailer; hash the entire archive.
                            val tail = ByteArray(65536)
                            while (decoded.read(tail) >= 0) { check(!cancelled.get()) { "cancelled" } }
                            while (input.read(tail) >= 0) { check(!cancelled.get()) { "cancelled" } }
                        }
                    }
                }
                check(downloaded == ARCHIVE_BYTES && hex(md.digest()) == ARCHIVE_HASH && seen.size == assets.size) { "checksum" }
                for (asset in assets) check(File(dir, asset.name + ".part").renameTo(File(dir, asset.name))) { "install" }
                check(!cancelled.get()) { "cancelled" }
                FileOutputStream(File(dir, "verified")).use { it.write(VERSION.toByteArray()); it.fd.sync() }
            } catch (error: Exception) {
                errorCode = if (cancelled.get()) null else error.javaClass.simpleName
                val reason = when (error) {
                    is java.net.UnknownHostException -> "模型站点暂不可达"
                    is javax.net.ssl.SSLException -> "模型站点安全连接失败"
                    is SecurityException -> "系统不允许下载连接"
                    is IllegalStateException -> when (error.message) {
                        "checksum" -> "模型校验不匹配"
                        "size" -> "模型大小不匹配"
                        "空间不足" -> "请留出至少280 MB可用空间"
                        "download" -> "模型站点未返回下载文件"
                        else -> "模型文件无法安装"
                    }
                    else -> "模型下载连接失败"
                }
                message = if (cancelled.get()) null else "$reason，请检查后重试"
            } finally {
                connection?.disconnect(); connection = null
                assets.forEach { File(dir, it.name + ".part").delete() }
                downloading.set(false); if (!ready(app)) downloaded = 0
            }
        }
    }
    fun cancel() { cancelled.set(true); connection?.disconnect() }
    @Synchronized fun remove(context: Context) {
        check(!CaptureState.recording && !inUse) { "请先停止录音并等待当前转写结束" }
        check(!downloading.get()) { "请先取消下载，等待下载停止后再删除" }
        check(directory(context).deleteRecursively()) { "无法删除离线语音模型" }
        message = null; errorCode = null; downloaded = 0
        // Keep local mode selected: missing model must never send queued audio to the cloud.
    }
    @Synchronized fun acquire(context: Context): File {
        check(ready(context)) { "离线模型不可用，原始录音保留在本机；请下载模型后继续" }
        check(!inUse) { "离线转写正在运行" }
        inUse = true
        return directory(context)
    }
    @Synchronized fun release() { inUse = false }

    private fun connect(address: String): HttpURLConnection {
        var url = URL(address)
        repeat(6) {
            check(!cancelled.get()) { "cancelled" }
            check(url.protocol == "https") { "https" }
            val remote = (url.openConnection() as HttpURLConnection).apply {
                instanceFollowRedirects = false; connectTimeout = 45_000; readTimeout = 90_000
            }
            connection = remote
            if (remote.responseCode in 300..399) {
                val location = remote.getHeaderField("Location") ?: error("redirect")
                url = URL(url, location); remote.disconnect()
            } else {
                check(remote.responseCode == 200) { "download" }
                return remote
            }
        }
        error("redirect")
    }
    private fun digest(file: File): String = MessageDigest.getInstance("SHA-256").let { md ->
        file.inputStream().use { input -> val buffer = ByteArray(65536); while (true) { val n = input.read(buffer); if (n < 0) break; md.update(buffer, 0, n) } }
        hex(md.digest())
    }
    private fun boundedDecoded(input: java.io.InputStream): java.io.InputStream = object : java.io.InputStream() {
        private var bytes = 0L
        private fun account(count: Int) {
            check(!cancelled.get()) { "cancelled" }
            bytes += count
            check(bytes <= 260_000_000) { "size" }
        }
        override fun read(): Int = input.read().also { if (it >= 0) account(1) }
        override fun read(buffer: ByteArray, offset: Int, length: Int): Int =
            input.read(buffer, offset, length).also { if (it > 0) account(it) }
        override fun close() = input.close()
    }
    private fun hex(bytes: ByteArray) = bytes.joinToString("") { "%02x".format(it) }
}
