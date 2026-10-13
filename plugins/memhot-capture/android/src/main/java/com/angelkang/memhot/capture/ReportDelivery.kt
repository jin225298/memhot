package com.angelkang.memhot.capture

object ReportDelivery {
    fun unseen(current: List<String>, seen: Set<String>): List<String> = current.distinct().filter { it !in seen }
}
