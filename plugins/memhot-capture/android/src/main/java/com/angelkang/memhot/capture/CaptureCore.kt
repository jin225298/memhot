package com.angelkang.memhot.capture

import java.io.ByteArrayOutputStream
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.util.ArrayDeque

/** Bounded 32 ms framing around a speech classifier. Keep onset audio and natural pauses. */
class SpeechGate {
    private val preRoll = ArrayDeque<ByteArray>()
    private var active: ByteArrayOutputStream? = null
    private var silenceFrames = 0
    private var voicedFrames = 0
    fun accept(frame: ByteArray, voiced: Boolean): ByteArray? {
        require(frame.size == 1024) { "Expected 32ms of 16kHz mono PCM16" }
        if (active == null && !voiced) {
            preRoll.addLast(frame.copyOf())
            while (preRoll.size > 16) preRoll.removeFirst()
            return null
        }
        if (active == null) {
            active = ByteArrayOutputStream()
            while (preRoll.isNotEmpty()) active!!.write(preRoll.removeFirst())
        }
        active!!.write(frame)
        if (voiced) { voicedFrames++; silenceFrames = 0 } else silenceFrames++
        return if (silenceFrames >= 38 || active!!.size() >= 384_000) finish() else null
    }

    fun finish(): ByteArray? {
        val result = active?.toByteArray()?.takeIf { voicedFrames >= 16 }
        active = null
        silenceFrames = 0
        voicedFrames = 0
        preRoll.clear()
        return result
    }
}

object QueuePolicy {
    const val MAX_COUNT = 512
    const val MAX_BYTES = 256L * 1024 * 1024
    const val DISK_RESERVE = 16L * 1024 * 1024

    fun canAppend(count: Int, bytes: Long, incoming: Long, freeDisk: Long): Boolean =
        count >= 0 && bytes >= 0 && incoming > 0 && incoming <= 960_044 &&
            count < MAX_COUNT && bytes <= MAX_BYTES - incoming &&
            freeDisk >= incoming + DISK_RESERVE
}

object PcmWav {
    fun encode(pcm: ByteArray): ByteArray {
        require(pcm.isNotEmpty() && pcm.size % 2 == 0 && pcm.size <= 960_000)
        val out = ByteBuffer.allocate(44 + pcm.size).order(ByteOrder.LITTLE_ENDIAN)
        out.put("RIFF".toByteArray()).putInt(36 + pcm.size).put("WAVEfmt ".toByteArray())
        out.putInt(16).putShort(1).putShort(1).putInt(16_000).putInt(32_000)
        out.putShort(2).putShort(16).put("data".toByteArray()).putInt(pcm.size).put(pcm)
        return out.array()
    }
}
