package com.angelkang.memhot.capture

import android.Manifest
import android.app.Activity
import android.content.Intent
import android.os.Build
import android.os.SystemClock
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import app.tauri.plugin.Invoke
import com.fasterxml.jackson.databind.ObjectMapper
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit

class CaptureTestActivity : Activity()

/** Exercises the real permission callbacks and AudioRecord; emulator has no host audio. */
@RunWith(AndroidJUnit4::class)
class PermissionStartTest {
    private val instrumentation = InstrumentationRegistry.getInstrumentation()
    private val context get() = instrumentation.targetContext

    @Test fun microphoneGrantBeforeResumeStartsAfterResume() = grantedBeforeResume(false)
    @Test fun notificationGrantBeforeResumeStartsAfterResume() = grantedBeforeResume(true)
    @Test fun leavingAppWhilePermissionCompletesCancelsStart() = grantedBeforeResume(false, "before")
    @Test fun leavingAppWhileWaitingForResumeCancelsStart() = grantedBeforeResume(false, "after")
    @Test fun missingResumeTimesOutWithoutStartingLater() = grantedBeforeResume(false, "timeout")

    private fun grantedBeforeResume(notification: Boolean, cancel: String? = null) {
        check(Build.FINGERPRINT.contains("generic") || Build.MODEL.contains("sdk_gphone"))
        instrumentation.uiAutomation.grantRuntimePermission(context.packageName, Manifest.permission.RECORD_AUDIO)
        if (Build.VERSION.SDK_INT >= 33) {
            instrumentation.uiAutomation.grantRuntimePermission(context.packageName, Manifest.permission.POST_NOTIFICATIONS)
        }
        val config = SecureConfig(context)
        val previous = config.read()
        config.write("https://permission-test.invalid", "emulator-only-fake-token-00000000000000000000000000000000", "permission-test")
        CaptureState.set(context, "idle")
        val activity = instrumentation.startActivitySync(Intent(context, CaptureTestActivity::class.java)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
        val answers = LinkedBlockingQueue<Pair<Long, String>>()
        lateinit var plugin: MemhotCapturePlugin
        val invoke = Invoke(1, "start", 10, 20, { callback, body -> answers.offer(callback to body) }, "{}", ObjectMapper())
        try {
            instrumentation.runOnMainSync {
                plugin = MemhotCapturePlugin(activity)
                plugin.onPause()
                if (cancel == "before") plugin.onStop()
                if (notification) plugin.notificationPermission(invoke) else plugin.microphonePermission(invoke)
                if (cancel == "after") plugin.onStop()
            }
            if (cancel != null) {
                val answer = answers.poll(if (cancel == "timeout") 6000 else 400, TimeUnit.MILLISECONDS)
                assertNotNull("Leaving or never resuming must finish the pending request", answer)
                assertEquals(20L, answer!!.first)
                instrumentation.runOnMainSync { plugin.onResume() }
                assertNull("A canceled start must never complete twice", answers.poll(400, TimeUnit.MILLISECONDS))
                assertFalse("Returning later must not silently start recording", CaptureState.recording)
                return
            }
            assertNull("An accepted permission must wait for resume, not reject or record in background", answers.poll(200, TimeUnit.MILLISECONDS))
            assertFalse(CaptureState.recording)
            instrumentation.runOnMainSync { plugin.onResume() }
            val answer = answers.poll(8, TimeUnit.SECONDS)
            assertNotNull("Permission flow did not finish after resume", answer)
            assertEquals("Recording command rejected: ${answer?.second}", 10L, answer!!.first)
            assertEquals("recording", JSONObject(answer.second).getString("state"))
            assertTrue(CaptureState.recording)
            instrumentation.runOnMainSync { plugin.onPause(); plugin.onStop() }
            SystemClock.sleep(1100)
            assertTrue("Background lifecycle must not stop an active recorder", CaptureState.recording)
            assertTrue(CaptureState.snapshot(context).getLong("elapsedMs") >= 1000)
            val stop = Invoke(2, "stop", 11, 21, { callback, body -> answers.offer(callback to body) }, "{}", ObjectMapper())
            instrumentation.runOnMainSync { plugin.stop(stop) }
            val stopped = answers.poll(6, TimeUnit.SECONDS)
            assertNotNull(stopped)
            assertEquals(11L, stopped!!.first)
            assertEquals("idle", JSONObject(stopped.second).getString("state"))
            assertFalse(CaptureState.recording)
        } finally {
            context.stopService(Intent(context, CaptureService::class.java))
            CaptureService.requestStopNow()?.join(5000)
            CaptureQueue(context).use { it.clearOwner(config.read().owner()) }
            config.write(previous.baseUrl, previous.token, previous.userId)
            instrumentation.runOnMainSync { activity.finish() }
        }
    }
}
