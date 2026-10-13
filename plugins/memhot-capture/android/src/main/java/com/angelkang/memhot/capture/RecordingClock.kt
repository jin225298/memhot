package com.angelkang.memhot.capture

/** Measures only the current recording session; callers supply a monotonic clock. */
class RecordingClock(private val nowMs: () -> Long) {
    private var startedAt: Long? = null

    @Synchronized fun start(at: Long = nowMs()) {
        if (startedAt == null) startedAt = at
    }

    @Synchronized fun stop() { startedAt = null }

    @Synchronized fun elapsedMs(): Long = startedAt?.let { (nowMs() - it).coerceAtLeast(0) } ?: 0
}
