package com.angelkang.memhot.capture

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assert.*
import org.junit.Assume.assumeTrue
import org.junit.Test
import org.junit.runner.RunWith

/** Explicit network test: public pinned artifacts and synthetic PCM, never microphone input. */
@RunWith(AndroidJUnit4::class)
class ModelInstallTest {
    @Test fun pinnedModelCanCancelInstallDecodeAndRemoveWithoutCloudFallback() {
        assumeTrue(InstrumentationRegistry.getArguments().getString("downloadModel") == "true")
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        LocalModel.remove(context)
        try {
            LocalModel.download(context)
            LocalModel.cancel()
            awaitDownload(context, 30_000)
            assertFalse(LocalModel.ready(context))
            assertTrue(LocalModel.directory(context).listFiles()?.none { it.extension == "part" } ?: true)
            LocalModel.download(context)
            awaitDownload(context, 900_000)
            assertTrue(LocalModel.status(context).toString(), LocalModel.ready(context))
            assertEquals(163_002_883L, LocalModel.status(context).getLong("downloadedBytes"))
            LocalModel.setMode(context, "local")
            LocalRecognizer(context).use { recognizer ->
                val transcript = recognizer.transcribe(PcmWav.encode(ByteArray(32_000)))
                assertTrue(transcript.length < 4096)
            }
            assertFalse(LocalModel.inUse)
            LocalModel.remove(context)
            assertFalse(LocalModel.ready(context))
            assertEquals("local", LocalModel.mode(context))
            assertFalse(LocalModel.directory(context).exists())
        } finally {
            LocalModel.cancel()
            awaitDownload(context, 35_000)
            LocalModel.remove(context)
            LocalModel.setMode(context, "cloud")
        }
    }
    private fun awaitDownload(context: android.content.Context, timeout: Long) {
        val end = android.os.SystemClock.elapsedRealtime() + timeout
        while (LocalModel.status(context).getString("state") == "downloading") {
            check(android.os.SystemClock.elapsedRealtime() < end) { "Model download timed out" }
            Thread.sleep(100)
        }
    }
}
