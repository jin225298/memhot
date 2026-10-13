package com.angelkang.memhot.capture

import org.junit.Assert.*
import org.junit.Test

class UploadScheduleTest {
    @Test fun newSpeechRequestsItsFirstAttemptAfterThreeSeconds() {
        assertEquals(3000L, UploadSchedule.delay(1000, 3000, null, true))
    }
    @Test fun repeatedSpeechCannotPostponeAnExistingAttempt() {
        // Initial scheduling at t=0 requests t=3s. Subsequent appends retain it.
        for (now in listOf(1000L, 2000L, 12_000L, 24_000L, 36_000L)) {
            assertNull(UploadSchedule.delay(now, 3000, 3000, true))
        }
    }
    @Test fun stoppingCanBringAnExistingAttemptForward() {
        assertEquals(1000L, UploadSchedule.delay(1000, 1000, 30_000, true))
    }
    @Test fun changingNetworkRequirementsRetainsTheEarlierDeadline() {
        assertEquals(2000L, UploadSchedule.delay(1000, 30_000, 3000, false))
        assertEquals(0L, UploadSchedule.delay(5000, 30_000, 3000, false))
    }
    @Test fun finishedBatchesUseTheNextRetryDeadline() {
        // The completed job is excluded by the dispatcher when scheduling its successor.
        assertEquals(60_000L, UploadSchedule.delay(5000, 60_000, null, true))
    }
}
