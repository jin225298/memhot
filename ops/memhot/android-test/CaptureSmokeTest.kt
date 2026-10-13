package com.angelkang.memhot

import android.Manifest
import android.app.NotificationManager
import android.content.Intent
import android.graphics.Bitmap
import android.os.Build
import android.os.SystemClock
import androidx.test.core.app.ActivityScenario
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.angelkang.memhot.capture.CaptureQueue
import com.angelkang.memhot.capture.CaptureService
import com.angelkang.memhot.capture.CaptureState
import com.angelkang.memhot.capture.SecureConfig
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File
import java.io.FileOutputStream

/** Emulator only. Does not enable host microphone or invoke any real provider. */
@RunWith(AndroidJUnit4::class)
class CaptureSmokeTest {
    private val instrumentation = InstrumentationRegistry.getInstrumentation()
    private val context get() = instrumentation.targetContext

    @Test fun foregroundMicrophoneSurvivesFiveMinutesBackgroundAndStops() {
        require(Build.FINGERPRINT.contains("generic") || Build.MODEL.contains("sdk_gphone")) {
            "This smoke test is restricted to an emulator"
        }
        shell("pm grant ${context.packageName} ${Manifest.permission.RECORD_AUDIO}")
        if (Build.VERSION.SDK_INT >= 33) shell("pm grant ${context.packageName} ${Manifest.permission.POST_NOTIFICATIONS}")
        val secure = SecureConfig(context)
        val previous = secure.read()
        secure.write("https://smoke.invalid", "emulator-only-fake-token-00000000000000000000000000000000", "smoke-user")
        val owner = secure.read().owner()
        CaptureQueue(context).use { it.clearOwner(owner) }
        try {
            // Tao exits the process when its last Activity is destroyed. Keep it alive until
            // instrumentation reports results; run with finishActivities=false and let AGP uninstall.
            val scenario = ActivityScenario.launch(MainActivity::class.java)
                scenario.onActivity { activity ->
                    val intent = Intent(activity, CaptureService::class.java)
                    if (Build.VERSION.SDK_INT >= 26) activity.startForegroundService(intent) else activity.startService(intent)
                }
                await("Microphone failed to start", 15_000) { CaptureState.recording }
                assertEquals("recording", CaptureState.snapshot(context).getString("state"))
                assertCaptureNotification()
                SystemClock.sleep(3000) // Let the WebView render before taking visual evidence.
                screenshot("foreground")
                shell("input keyevent KEYCODE_HOME")
                SystemClock.sleep(1000)
                val started = SystemClock.elapsedRealtime()
                val duration = InstrumentationRegistry.getArguments().getString("captureSmokeDurationMs")
                    ?.toLongOrNull() ?: 300_000L
                require(duration in 1000..300_000)
                while (SystemClock.elapsedRealtime() - started < duration) {
                    assertTrue("Recording stopped during background smoke", CaptureState.recording)
                    assertCaptureNotification()
                    SystemClock.sleep(1000)
                }
                shell("cmd statusbar expand-notifications")
                SystemClock.sleep(1000)
                screenshot("background-notification")
                shell("cmd statusbar collapse")
                // Exercise the same explicit stop action used by the persistent notification.
                val stop = Intent(context, CaptureService::class.java).setAction("com.angelkang.memhot.STOP_CAPTURE")
                context.startService(stop)
                await("Microphone did not stop", 10_000) {
                    !CaptureState.recording && CaptureState.snapshot(context).getString("state") == "idle" &&
                        context.getSystemService(NotificationManager::class.java).activeNotifications.none { it.id == 17340 }
                }
                assertEquals("idle", CaptureState.snapshot(context).getString("state"))
                assertFalse(context.getSystemService(NotificationManager::class.java).activeNotifications.any { it.id == 17340 })
                screenshot("stopped")
                println("MEMHOT_SMOKE backgroundMs=$duration stopped=idle notificationRemoved=true")
        } finally {
            context.stopService(Intent(context, CaptureService::class.java))
            CaptureService.requestStopNow()?.join(5000)
            CaptureQueue(context).use { it.clearOwner(owner) }
            secure.write(previous.baseUrl, previous.token, previous.userId)
        }
    }

    private fun assertCaptureNotification() {
        assertTrue("Persistent microphone notification missing", context.getSystemService(NotificationManager::class.java)
            .activeNotifications.any { it.id == 17340 && it.isOngoing })
    }

    private fun await(message: String, timeoutMs: Long, condition: () -> Boolean) {
        val start = SystemClock.elapsedRealtime()
        while (!condition() && SystemClock.elapsedRealtime() - start < timeoutMs) SystemClock.sleep(100)
        assertTrue(message, condition())
    }

    private fun shell(command: String) {
        instrumentation.uiAutomation.executeShellCommand(command).use { descriptor ->
            java.io.FileInputStream(descriptor.fileDescriptor).use { it.readBytes() }
        }
    }

    private fun screenshot(stage: String) {
        val image = instrumentation.uiAutomation.takeScreenshot() ?: return
        val file = File(context.getExternalFilesDir(null), "capture-smoke-$stage.png")
        FileOutputStream(file).use { image.compress(Bitmap.CompressFormat.PNG, 100, it) }
        image.recycle()
        // AGP uninstalls test applications after connected tests; retain only these emulator screenshots.
        shell("cp ${file.absolutePath} /data/local/tmp/memhot-capture-smoke-$stage.png")
    }
}
