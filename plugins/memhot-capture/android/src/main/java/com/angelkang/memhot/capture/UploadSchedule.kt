package com.angelkang.memhot.capture

/** Null keeps the pending job, including an overdue job waiting for connectivity. */
object UploadSchedule {
    fun delay(now: Long, requested: Long, pendingDeadline: Long?, sameNetwork: Boolean): Long? {
        val requestedDeadline = now + maxOf(0L, requested)
        if (sameNetwork && pendingDeadline != null && pendingDeadline <= requestedDeadline) return null
        return maxOf(0L, minOf(requestedDeadline, pendingDeadline ?: requestedDeadline) - now)
    }
}
