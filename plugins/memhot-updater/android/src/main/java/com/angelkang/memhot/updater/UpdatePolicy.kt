package com.angelkang.memhot.updater

import java.io.File
import java.security.MessageDigest
import java.text.ParsePosition
import java.text.SimpleDateFormat
import java.util.Locale

data class UpdateRelease(val schemaVersion: Int, val packageName: String, val version: String,
    val versionCode: Long, val minSdk: Int, val abi: String, val url: String, val size: Long,
    val sha256: String, val notes: List<String>, val publishedAt: String) {
    fun validate(deviceSdk: Int) {
        require(schemaVersion == 1 && packageName == UpdatePolicy.PACKAGE)
        require(version.matches(Regex("(0|[1-9][0-9]{0,5})\\.(0|[1-9][0-9]{0,5})\\.(0|[1-9][0-9]{0,5})")))
        require(versionCode in 1..Int.MAX_VALUE.toLong())
        require(minSdk in 24..deviceSdk && abi == "arm64-v8a")
        require(url == "${UpdatePolicy.ORIGIN}/downloads/memhot-$version-arm64.apk")
        require(size in 1..UpdatePolicy.MAX_APK_BYTES)
        require(sha256.matches(Regex("[0-9a-f]{64}")))
        require(notes.size <= 16 && notes.all { it.length <= 500 })
        val match = Regex("([0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2})(?:\\.([0-9]{1,9}))?(Z|[+-][0-9]{2}:[0-9]{2})")
            .matchEntire(publishedAt) ?: throw IllegalArgumentException("Invalid publication date")
        val fraction = match.groupValues[2]
        val format = if (fraction.isEmpty()) "yyyy-MM-dd'T'HH:mm:ssXXX" else "yyyy-MM-dd'T'HH:mm:ss.SSSXXX"
        val normalized = match.groupValues[1] + (if (fraction.isEmpty()) "" else "." + fraction.padEnd(3, '0').take(3)) + match.groupValues[3]
        val parser = SimpleDateFormat(format, Locale.ROOT).apply { isLenient = false }
        val position = ParsePosition(0)
        require(parser.parse(normalized, position) != null && position.index == normalized.length)
    }
}
data class ApkIdentity(val packageName: String, val versionName: String, val versionCode: Long, val signers: Set<String>)
object UpdatePolicy {
    const val PACKAGE = "com.angelkang.memhot"
    const val ORIGIN = "https://memhot.angelkang.com"
    const val MANIFEST_URL = "$ORIGIN/updates/android-arm64.json"
    const val MAX_MANIFEST_BYTES = 16 * 1024
    const val MAX_APK_BYTES = 200L * 1024 * 1024

    fun verifyBytes(file: File, release: UpdateRelease) {
        require(file.isFile && file.length() == release.size) { "更新包大小不符，请重新下载" }
        val digest = MessageDigest.getInstance("SHA-256")
        file.inputStream().use { input ->
            val buffer = ByteArray(65536)
            var count = 0L
            while (true) {
                val n = input.read(buffer)
                if (n < 0) break
                count += n
                require(count <= release.size) { "更新包大小不符，请重新下载" }
                digest.update(buffer, 0, n)
            }
            require(count == release.size) { "更新包下载不完整，请重新下载" }
        }
        require(hex(digest.digest()) == release.sha256) { "更新包校验失败，请重新下载" }
    }
    fun verifyIdentity(installed: ApkIdentity, archive: ApkIdentity, release: UpdateRelease) {
        require(installed.packageName == PACKAGE && archive.packageName == PACKAGE && release.packageName == PACKAGE) { "更新包应用不符" }
        require(archive.versionName == release.version && archive.versionCode == release.versionCode) { "更新包版本不符" }
        require(archive.versionCode > installed.versionCode) { "无需安装当前版本或旧版本" }
        require(installed.signers.isNotEmpty() && archive.signers == installed.signers) { "更新包签名与当前应用不符" }
    }
    fun hex(bytes: ByteArray) = bytes.joinToString("") { "%02x".format(it) }
}
