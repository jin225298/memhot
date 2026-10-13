package com.angelkang.memhot.updater

import android.os.Build
import android.os.SystemClock
import java.io.ByteArrayOutputStream
import java.net.URL
import javax.net.ssl.HttpsURLConnection

object UpdateNetwork {
    private fun connection(url: String, method: String) = (URL(url).openConnection() as HttpsURLConnection).apply {
        instanceFollowRedirects = false
        connectTimeout = 10000
        readTimeout = 15000
        requestMethod = method
        setRequestProperty("Accept-Encoding", "identity")
        useCaches = false
    }

    fun fetchManifest(): UpdateRelease {
        val connection = connection(UpdatePolicy.MANIFEST_URL, "GET")
        val deadline = SystemClock.elapsedRealtime() + 30000
        try {
            require(connection.responseCode == 200) { "无法获取更新信息，请稍后重试" }
            require(connection.contentLengthLong <= UpdatePolicy.MAX_MANIFEST_BYTES)
            val output = ByteArrayOutputStream()
            connection.inputStream.use { input ->
                val buffer = ByteArray(4096)
                while (true) {
                    require(SystemClock.elapsedRealtime() < deadline) { "检查更新超时，请重试" }
                    val count = input.read(buffer)
                    if (count < 0) break
                    require(output.size() + count <= UpdatePolicy.MAX_MANIFEST_BYTES)
                    output.write(buffer, 0, count)
                }
            }
            // Reject malformed UTF-8 rather than silently accepting replacement characters.
            val text = Charsets.UTF_8.newDecoder().decode(java.nio.ByteBuffer.wrap(output.toByteArray())).toString()
            return ManifestCodec.parse(text, Build.VERSION.SDK_INT)
        } finally { connection.disconnect() }
    }

    fun preflight(release: UpdateRelease) {
        release.validate(Build.VERSION.SDK_INT)
        val connection = connection(release.url, "HEAD")
        try {
            require(connection.responseCode == 200 && connection.contentLengthLong == release.size) {
                "更新包地址或大小异常，请稍后重新检查更新"
            }
            require(connection.getHeaderField("Content-Encoding").let { it == null || it == "identity" })
        } finally { connection.disconnect() }
    }
}
