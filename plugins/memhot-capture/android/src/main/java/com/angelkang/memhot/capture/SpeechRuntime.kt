package com.angelkang.memhot.capture

import android.content.Context
import com.k2fsa.sherpa.onnx.OfflineModelConfig
import com.k2fsa.sherpa.onnx.OfflineRecognizer
import com.k2fsa.sherpa.onnx.OfflineRecognizerConfig
import com.k2fsa.sherpa.onnx.OfflineSenseVoiceModelConfig
import com.k2fsa.sherpa.onnx.SileroVadModelConfig
import com.k2fsa.sherpa.onnx.Vad
import com.k2fsa.sherpa.onnx.VadModelConfig
import java.io.Closeable

fun pcmFloats(pcm: ByteArray, offset: Int = 0): FloatArray = FloatArray((pcm.size - offset) / 2) { i ->
    ((pcm[offset + i * 2].toInt() and 255) or (pcm[offset + i * 2 + 1].toInt() shl 8)).toShort() / 32768f
}

/** A real speech classifier; the accumulator alone decides pre-roll and endpoint timing. */
class SpeechDetector(context: Context) : Closeable {
    private val vad = Vad(context.assets, VadModelConfig(
        sileroVadModelConfig = SileroVadModelConfig(model = "silero_vad.onnx", windowSize = 512),
        sampleRate = 16000, numThreads = 1, provider = "cpu", debug = false,
    ))
    fun speech(frame: ByteArray): Boolean = vad.compute(pcmFloats(frame)) >= 0.5f
    override fun close() = vad.release()
}

class LocalRecognizer(context: Context) : Closeable {
    private val recognizer: OfflineRecognizer
    init {
        val dir = LocalModel.acquire(context)
        try {
            recognizer = OfflineRecognizer(config = OfflineRecognizerConfig(modelConfig = OfflineModelConfig(
                senseVoice = OfflineSenseVoiceModelConfig(model = java.io.File(dir, "model.int8.onnx").path,
                    language = "auto", useInverseTextNormalization = true),
                tokens = java.io.File(dir, "tokens.txt").path, numThreads = 1, provider = "cpu", debug = false,
            )))
        } catch (error: Throwable) { LocalModel.release(); throw error }
    }
    fun transcribe(wav: ByteArray): String {
        require(wav.size > 44 && String(wav.copyOfRange(0, 4)) == "RIFF")
        val stream = recognizer.createStream()
        try {
            stream.acceptWaveform(pcmFloats(wav, 44), 16000)
            recognizer.decode(stream)
            val result = recognizer.getResult(stream)
            return result.text.replace(Regex("<\\|[^|]+\\|>"), "").trim()
        } finally { stream.release() }
    }
    override fun close() { try { recognizer.release() } finally { LocalModel.release() } }
}
