package com.angelkang.memhot.capture

import org.junit.Assert.assertEquals
import org.junit.Test

class RecordingClockTest {
    @Test fun includesBackgroundTimeWithoutTimerTicks() {
        var monotonic = 1000L
        val clock = RecordingClock { monotonic }
        assertEquals(0L, clock.elapsedMs())
        clock.start()
        monotonic += 93_000
        assertEquals(93_000L, clock.elapsedMs())
    }

    @Test fun actualStartPrecedesStatusAndDuplicateStartDoesNotResetIt() {
        var monotonic = 5000L
        val clock = RecordingClock { monotonic }
        clock.start(4700L)
        assertEquals(300L, clock.elapsedMs())
        clock.start()
        monotonic += 600
        assertEquals(900L, clock.elapsedMs())
    }

    @Test fun stoppedAndNewSessionsNeverReuseAnOldDuration() {
        var monotonic = 10L
        val clock = RecordingClock { monotonic }
        clock.start()
        monotonic += 5000
        clock.stop()
        monotonic += 20_000
        assertEquals(0L, clock.elapsedMs())
        clock.start()
        monotonic += 200
        assertEquals(200L, clock.elapsedMs())
        assertEquals(0L, RecordingClock { monotonic }.elapsedMs())
    }
}
