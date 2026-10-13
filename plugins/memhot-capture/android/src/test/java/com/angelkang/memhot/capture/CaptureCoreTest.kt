package com.angelkang.memhot.capture

import org.junit.Assert.*
import org.junit.Test

class CaptureCoreTest {
    @Test fun deliveryDeduplicatesReportIds() {
        assertEquals(listOf("new"), ReportDelivery.unseen(listOf("old", "new", "new"), setOf("old")))
    }
    @Test fun tokenRotationKeepsQueueOwnership() {
        val config = DeviceConfig("https://memhot.example", "first-token", "alice")
        assertEquals(config.owner(), config.copy(token = "rotated-token").owner())
        assertNotEquals(config.owner(), config.copy(userId = "bob").owner())
        assertNotEquals(config.owner(), config.copy(baseUrl = "https://other.example").owner())
    }

    @Test fun silenceIsNotQueuedAndSpeechKeepsPreRoll() {
        val gate = SpeechGate()
        val silence = ByteArray(1024)
        repeat(30) { assertNull(gate.accept(silence, false)) }
        val speech = ByteArray(1024) { if (it % 2 == 0) 0 else 30 }
        repeat(20) { assertNull(gate.accept(speech, true)) }
        var result: ByteArray? = null
        repeat(39) { gate.accept(silence, false)?.let { result = it } }
        assertNotNull(result)
        assertTrue(result!!.size > speech.size * 20)
        assertNull(gate.finish())
    }

    @Test fun continuousSpeechHardCutsWithinTwelveSeconds() {
        val gate = SpeechGate()
        val speech = ByteArray(1024) { if (it % 2 == 0) 0 else 30 }
        var segments = 0
        repeat(2100) {
            gate.accept(speech, true)?.let { chunk ->
                assertTrue(chunk.size <= 384_000)
                segments++
            }
        }
        assertEquals(5, segments)
    }

    @Test fun queueBoundsRefuseNewAudioWithoutDiscardingOldAudio() {
        assertTrue(QueuePolicy.canAppend(100, 1_000, 960_000, 20_000_000))
        assertFalse(QueuePolicy.canAppend(512, 1_000, 960_000, 20_000_000))
        assertFalse(QueuePolicy.canAppend(1, QueuePolicy.MAX_BYTES - 10, 960_000, 20_000_000))
        assertFalse(QueuePolicy.canAppend(1, 0, 960_000, 1_000_000))
        assertFalse(QueuePolicy.canAppend(1, 0, -1, 20_000_000))
    }

    @Test fun wavHeaderMatchesPayloadWithoutDoubleWrapping() {
        val wav = PcmWav.encode(ByteArray(32_000))
        assertEquals(32_044, wav.size)
        assertEquals("RIFF", String(wav.copyOfRange(0, 4)))
        assertEquals("WAVE", String(wav.copyOfRange(8, 12)))
        assertEquals(0x80.toByte(), wav[24]) // 16000 little endian
        assertEquals(0x3e.toByte(), wav[25])
    }
}
