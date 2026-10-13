package com.angelkang.memhot.capture

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class UploadStateTest {
    @Test fun dispatcherKeepsTheEarliestPendingJobWhenSpeechContinues() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val scheduler = context.getSystemService(android.app.job.JobScheduler::class.java)
        val id = 17341
        scheduler.cancel(id)
        try {
            UploadDispatcher.schedule(context, 3000)
            val initial = scheduler.getPendingJob(id)!!.extras.getLong("uploadDueAt")
            repeat(5) { UploadDispatcher.schedule(context, 30_000) }
            assertEquals(initial, scheduler.getPendingJob(id)!!.extras.getLong("uploadDueAt"))
            UploadDispatcher.schedule(context, 1000)
            assertTrue(scheduler.getPendingJob(id)!!.extras.getLong("uploadDueAt") < initial)
        } finally { scheduler.cancel(id) }
    }
    @Test fun synchronizationErrorsStayWithTheirAccountAndClearAfterSuccess() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val first = "synthetic-upload-state-a"
        val second = "synthetic-upload-state-b"
        try {
            CaptureState.syncFailed(context, first, "额度不足")
            CaptureState.syncFailed(context, second, "网络中断")
            assertEquals("额度不足", CaptureState.syncError(context, first))
            assertEquals("网络中断", CaptureState.syncError(context, second))
            CaptureState.uploaded(context, first)
            assertNull(CaptureState.syncError(context, first))
            assertEquals("网络中断", CaptureState.syncError(context, second))
        } finally {
            CaptureState.clearSyncError(context, first)
            CaptureState.clearSyncError(context, second)
        }
    }

    @Test fun cloudAudioIsRetainedUntilTextAcknowledgementThenRemovedDurably() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val owner = DeviceConfig("https://queue-test.invalid", "test", "cloud-ack-test").owner()
        CaptureQueue(context).use { it.clearOwner(owner) }
        try {
            CaptureQueue(context).use { queue ->
                queue.append(ByteArray(32000), 1_800_000_000_000, owner)
                val item = queue.next(owner)!!
                queue.retry(item)
                assertTrue(item.file.exists())
                queue.unblock(owner, false)
                queue.acknowledged(queue.next(owner)!!)
                assertFalse(item.file.exists())
            }
            CaptureQueue(context).use { queue ->
                queue.recover()
                assertEquals(0, queue.pending(owner))
                assertNull(queue.next(owner))
            }
        } finally { CaptureQueue(context).use { it.clearOwner(owner) } }
    }
}
