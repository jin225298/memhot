package com.angelkang.memhot.capture

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class SpeechRuntimeTest {
    @Test fun emptyResultBackoffIsBoundedAndRealTextResetsIt() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val owner = "synthetic-backoff"
        SpeechBackoff.result(context, owner, false)
        repeat(2) { SpeechBackoff.result(context, owner, true) }
        assertEquals(0L, SpeechBackoff.delay(context, owner))
        SpeechBackoff.result(context, owner, true)
        assertTrue(SpeechBackoff.delay(context, owner) in 1..30_000)
        repeat(20) { SpeechBackoff.result(context, owner, true) }
        assertTrue(SpeechBackoff.delay(context, owner) in 1..300_000)
        SpeechBackoff.result(context, owner, false)
        assertEquals(0L, SpeechBackoff.delay(context, owner))
    }
    @Test fun bundledVadLoadsAndSyntheticSilenceNeverCreatesAudio() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val gate = SpeechGate()
        SpeechDetector(context).use { detector ->
            repeat(400) {
                val silent = ByteArray(1024)
                val speech = detector.speech(silent)
                assertFalse(speech)
                assertNull(gate.accept(silent, speech))
            }
        }
        assertNull(gate.finish())
    }
    @Test fun locallyCommittedTextSurvivesRestartWithoutRawAudio() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val owner = DeviceConfig("https://queue-test.invalid", "test", "local-text-test").owner()
        CaptureQueue(context).use { it.clearOwner(owner) }
        try {
            CaptureQueue(context).use { queue ->
                queue.append(ByteArray(32000), 1_800_000_000_000, owner, "local")
                assertTrue(queue.needsLocalDecode(owner))
                val item = queue.next(owner)!!
                assertEquals("local", item.mode)
                queue.transcribed(item, "合成测试文字")
                assertFalse(queue.needsLocalDecode(owner))
                assertFalse(item.file.exists())
            }
            CaptureQueue(context).use { queue ->
                queue.recover()
                val restored = queue.next(owner)!!
                assertEquals("合成测试文字", restored.text)
                assertEquals(1000L, restored.durationMs)
                assertFalse(restored.file.exists())
                queue.block(restored)
                assertNull(queue.next(owner))
                queue.unblock(owner, true)
                val retry = queue.retryKey(restored)
                assertNotNull(retry)
                assertEquals(retry, queue.retryKey(restored))
                queue.acknowledged(restored)
            }
        } finally { CaptureQueue(context).use { it.clearOwner(owner) } }
    }
}
