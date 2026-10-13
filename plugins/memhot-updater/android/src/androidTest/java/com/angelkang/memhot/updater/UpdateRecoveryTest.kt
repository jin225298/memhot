package com.angelkang.memhot.updater

import android.content.Context
import android.os.Build
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class UpdateRecoveryTest {
    private fun release() = UpdateRelease(1, "com.angelkang.memhot", "0.3.3", 1006, 24,
        "arm64-v8a", "https://memhot.angelkang.com/downloads/memhot-0.3.3-arm64.apk", 3,
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad", listOf("更新"), "2026-10-12T00:00:00Z")

    private fun rejects(text: String) {
        try { ManifestCodec.parse(text, Build.VERSION.SDK_INT); fail("Invalid manifest accepted") }
        catch (_: Exception) { }
    }
    @Test fun strictManifestRejectsCoercionUnknownFieldsAndOversizedInput() {
        val json = ManifestCodec.json(release())
        assertEquals(1006L, ManifestCodec.parse(json.toString(), Build.VERSION.SDK_INT).versionCode)
        rejects(JSONObject(json.toString()).put("versionCode", "1006").toString())
        rejects(JSONObject(json.toString()).put("size", 3.5).toString())
        rejects(JSONObject(json.toString()).put("extra", true).toString())
        rejects(JSONObject(json.toString()).put("notes", listOf(4)).toString())
        rejects(json.toString() + " true")
        rejects(" ".repeat(16385) + json.toString())
    }
    @Test fun strictManifestRejectsDuplicateKeysAndNonJsonSyntax() {
        val text = ManifestCodec.json(release()).toString()
        rejects(text.replaceFirst("{", "{\"schemaVersion\":1,"))
        rejects(text.replace("\"schemaVersion\"", "'schemaVersion'"))
    }
    @Test fun validatedReleasePersistsAcrossEngineInstances() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val prefs = context.getSharedPreferences("memhot.updater.v1", Context.MODE_PRIVATE)
        try {
            prefs.edit().clear().putString("release", ManifestCodec.json(release()).toString())
                .putString("phase", "available").putLong("downloadId", -1).commit()
            val first = UpdateEngine(context).getStatus()
            val recovered = UpdateEngine(context).getStatus()
            assertEquals("available", first.getString("phase"))
            assertEquals("available", recovered.getString("phase"))
            assertEquals(1006L, recovered.getJSONObject("release").getLong("versionCode"))
        } finally { prefs.edit().clear().commit() }
    }
    @Test fun corruptPersistedUrlNeverBecomesDownloadMetadata() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val prefs = context.getSharedPreferences("memhot.updater.v1", Context.MODE_PRIVATE)
        try {
            prefs.edit().clear().putString("release", ManifestCodec.json(release()).put("url", "https://evil.example/update.apk").toString())
                .putString("phase", "available").commit()
            val recovered = UpdateEngine(context).getStatus()
            assertEquals("idle", recovered.getString("phase"))
            assertTrue(recovered.isNull("release"))
        } finally { prefs.edit().clear().commit() }
    }
    @Test fun missingSystemDownloadRecoversToRetryableFailure() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val prefs = context.getSharedPreferences("memhot.updater.v1", Context.MODE_PRIVATE)
        try {
            prefs.edit().clear().putString("release", ManifestCodec.json(release()).toString())
                .putString("phase", "downloading").putLong("downloadId", Long.MAX_VALUE).commit()
            val recovered = UpdateEngine(context).getStatus()
            assertEquals("failed", recovered.getString("phase"))
            assertFalse(recovered.isNull("error"))
            assertEquals("available", UpdateEngine(context).cancelDownload().getString("phase"))
        } finally { prefs.edit().clear().commit() }
    }
}
