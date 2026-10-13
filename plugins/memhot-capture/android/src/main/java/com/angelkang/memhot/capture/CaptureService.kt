package com.angelkang.memhot.capture

import android.Manifest
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Intent
import android.content.pm.PackageManager
import android.content.pm.ServiceInfo
import android.media.AudioFormat
import android.media.AudioManager
import android.media.AudioRecord
import android.media.AudioRecordingConfiguration
import android.media.MediaRecorder
import android.os.Build
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.os.PowerManager
import android.os.SystemClock
import java.util.concurrent.atomic.AtomicBoolean

class CaptureService : Service() {
    private val running = AtomicBoolean(false)
    @Volatile private var recorder: AudioRecord? = null
    @Volatile private var requestedStop = false
    @Volatile private var interruptReason: String? = null
    private var wakeLock: PowerManager.WakeLock? = null
    @Volatile private var captureThread: Thread? = null
    private var recordingCallback: AudioManager.AudioRecordingCallback? = null
    private val handler = Handler(Looper.getMainLooper())
    private val uploadTick = object : Runnable {
        override fun run() {
            if (running.get()) {
                UploadDispatcher.upload(applicationContext)
                handler.postDelayed(this, 60_000)
            }
        }
    }

    override fun onBind(intent: Intent?): IBinder? = null
    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == STOP) { requestedStop = true; stopSelf(); return START_NOT_STICKY }
        if (running.get()) return START_NOT_STICKY
        try {
            val notification = notification("正在录音，语音片段将按所选方式转写")
            if (Build.VERSION.SDK_INT >= 29) startForeground(NOTIFICATION_ID, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE)
            else startForeground(NOTIFICATION_ID, notification)
        } catch (_: Exception) {
            CaptureState.set(this, "error", "系统拒绝启动麦克风前台服务，请在应用内重新开始")
            stopSelf()
            return START_NOT_STICKY
        }
        requestedStop = false
        interruptReason = null
        running.set(true)
        activeService = this
        captureThread = Thread({ capture() }, "memhot-audio-record").apply { start() }
        return START_NOT_STICKY // Never silently restart a microphone after process death.
    }

    private fun capture() {
        val gate = SpeechGate()
        var samplesRead = 0L
        val startedAt = System.currentTimeMillis()
        var failure: String? = null
        var owner: String? = null
        var queue: CaptureQueue? = null
        var detector: SpeechDetector? = null
        var captureMode = "cloud"
        try {
            check(checkSelfPermission(Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED) {
                "麦克风权限已撤销，请重新授权"
            }
            val config = SecureConfig(this).read()
            check(config.token.isNotEmpty()) { "请先登录" }
            val captureOwner = config.owner()
            captureMode = LocalModel.mode(this)
            check(captureMode != "local" || LocalModel.ready(this)) { "请先下载离线语音模型" }
            val speechDetector = SpeechDetector(this)
            detector = speechDetector
            owner = captureOwner
            val storage = CaptureQueue(this).apply { recover() }
            queue = storage
            val minimum = AudioRecord.getMinBufferSize(16_000, AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT)
            check(minimum > 0) { "设备不支持16kHz单声道录音" }
            val audio = AudioRecord.Builder()
                .setAudioSource(MediaRecorder.AudioSource.VOICE_RECOGNITION)
                .setAudioFormat(AudioFormat.Builder().setSampleRate(16_000)
                    .setChannelMask(AudioFormat.CHANNEL_IN_MONO).setEncoding(AudioFormat.ENCODING_PCM_16BIT).build())
                .setBufferSizeInBytes(maxOf(minimum * 2, 32_000)).build()
            recorder = audio
            check(audio.state == AudioRecord.STATE_INITIALIZED) { "麦克风初始化失败" }
            audio.startRecording()
            check(audio.recordingState == AudioRecord.RECORDSTATE_RECORDING) { "麦克风被占用，未开始录音" }
            val recordingStartedAt = SystemClock.elapsedRealtime()
            registerSilencingCallback(audio)
            // Capture needs CPU to drain AudioRecord while screen is off. No JS, waveform or local LLM runs here.
            wakeLock = getSystemService(PowerManager::class.java).newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "Memhot:AudioCapture").apply {
                setReferenceCounted(false); acquire()
            }
            CaptureState.recordingStarted(recordingStartedAt)
            CaptureState.set(this, "recording")
            handler.postDelayed(uploadTick, 60_000)
            val frame = ByteArray(1024)
            var filled = 0
            var nextPolicyCheck = 0L
            val audioManager = getSystemService(AudioManager::class.java)
            while (running.get()) {
                // Permission/mode checks cross Binder. Once per second avoids dozens
                // of IPC calls per second; recording callbacks still stop silenced audio immediately.
                val elapsed = android.os.SystemClock.elapsedRealtime()
                if (elapsed >= nextPolicyCheck) {
                    nextPolicyCheck = elapsed + 1000
                    check(checkSelfPermission(Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED) { "麦克风权限已撤销，录音已停止" }
                    check(audioManager.mode == AudioManager.MODE_NORMAL) { "通话正在使用麦克风，录音已暂停；请结束通话后重新开始" }
                }
                val read = audio.read(frame, filled, frame.size - filled, AudioRecord.READ_BLOCKING)
                check(read > 0) { "麦克风设备中断，已保留已提交的片段；请重新开始" }
                filled += read
                if (filled < frame.size) continue
                samplesRead += 512
                gate.accept(frame, speechDetector.speech(frame))?.let { pcm ->
                    val capturedAt = startedAt + samplesRead * 1000 / 16_000 - pcm.size * 1000L / 32_000
                    storage.append(pcm, capturedAt, captureOwner, captureMode)
                    UploadDispatcher.schedule(this)
                }
                filled = 0
            }
        } catch (error: Throwable) {
            failure = interruptReason ?: when (error) {
                is SecurityException -> "麦克风权限被撤销或系统限制录音，请打开应用检查权限"
                is IllegalStateException -> error.message?.takeIf { it.length < 180 } ?: "录音中断，请检查麦克风和存储空间"
                else -> "录音中断，请检查麦克风和存储空间；已有片段已保留"
            }
        } finally {
            if (requestedStop) failure = null
            try {
                gate.finish()?.let { pcm ->
                    queue?.let { storage -> owner?.let { identity ->
                        val at = startedAt + samplesRead * 1000 / 16_000 - pcm.size * 1000L / 32_000
                        storage.append(pcm, at, identity, captureMode)
                    } }
                }
            } catch (_: Exception) { failure = "录音队列或存储空间已满，最后片段未能保存；请检查后重新开始" }
            running.set(false)
            handler.removeCallbacks(uploadTick)
            recordingCallback?.let { callback ->
                try { getSystemService(AudioManager::class.java).unregisterAudioRecordingCallback(callback) } catch (_: Exception) { }
            }
            try { recorder?.stop() } catch (_: Exception) { }
            recorder?.release(); recorder = null
            try { detector?.close() } catch (_: Throwable) { }
            wakeLock?.let { if (it.isHeld) it.release() }; wakeLock = null
            queue?.close()
            CaptureState.recordingStopped()
            if (activeService === this) activeService = null
            if (failure != null) {
                CaptureState.set(this, "paused", failure)
                getSystemService(NotificationManager::class.java).notify(ERROR_ID, notification(failure))
            } else CaptureState.set(this, "idle", failure)
            UploadDispatcher.schedule(this, 1000)
            stopForeground(STOP_FOREGROUND_REMOVE)
            stopSelf()
        }
    }

    private fun registerSilencingCallback(audio: AudioRecord) {
        if (Build.VERSION.SDK_INT < 29) return
        val callback = object : AudioManager.AudioRecordingCallback() {
            override fun onRecordingConfigChanged(configs: MutableList<AudioRecordingConfiguration>) {
                if (configs.any { it.clientAudioSessionId == audio.audioSessionId && it.isClientSilenced }) {
                    interruptReason = "系统已静音麦克风，录音已暂停；请检查通话或麦克风隐私开关"
                    running.set(false)
                    try { audio.stop() } catch (_: Exception) { }
                }
            }
        }
        recordingCallback = callback
        getSystemService(AudioManager::class.java).registerAudioRecordingCallback(callback, handler)
    }

    private fun notification(text: String): Notification {
        val manager = getSystemService(NotificationManager::class.java)
        if (Build.VERSION.SDK_INT >= 26) manager.createNotificationChannel(NotificationChannel(CHANNEL, "Memhot 录音", NotificationManager.IMPORTANCE_LOW))
        val stop = PendingIntent.getService(this, 1, Intent(this, CaptureService::class.java).setAction(STOP), PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE)
        val launch = packageManager.getLaunchIntentForPackage(packageName)
        val content = launch?.let { PendingIntent.getActivity(this, 2, it, PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE) }
        val builder = if (Build.VERSION.SDK_INT >= 26) Notification.Builder(this, CHANNEL) else Notification.Builder(this)
        return builder.setContentTitle("Memhot").setContentText(text).setSmallIcon(android.R.drawable.ic_btn_speak_now)
            .setContentIntent(content).setOngoing(true).setOnlyAlertOnce(true)
            .addAction(Notification.Action.Builder(null, "停止录音", stop).build()).build()
    }

    override fun onDestroy() {
        requestedStop = true
        running.set(false)
        handler.removeCallbacks(uploadTick)
        try { recorder?.stop() } catch (_: Exception) { }
        super.onDestroy()
    }

    companion object {
        @Volatile private var activeService: CaptureService? = null
        fun requestStopNow(): Thread? {
            val service = activeService ?: return null
            service.requestedStop = true
            service.running.set(false)
            try { service.recorder?.stop() } catch (_: Exception) { }
            return service.captureThread
        }
        private const val STOP = "com.angelkang.memhot.STOP_CAPTURE"
        private const val CHANNEL = "memhot-capture"
        private const val NOTIFICATION_ID = 17340
        private const val ERROR_ID = 17342
    }
}
