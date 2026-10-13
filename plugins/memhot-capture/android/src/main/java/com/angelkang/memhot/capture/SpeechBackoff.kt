package com.angelkang.memhot.capture

import android.content.Context

/** Empty recognition is normal. Repeated empties delay requests, never stop the microphone. */
object SpeechBackoff {
    private fun prefs(context: Context) = context.getSharedPreferences("memhot-speech-backoff", Context.MODE_PRIVATE)
    fun count(context: Context, owner: String) = prefs(context).getInt("count:$owner", 0)
    fun delay(context: Context, owner: String) = maxOf(0L, prefs(context).getLong("until:$owner", 0) - System.currentTimeMillis())
    @Synchronized fun result(context: Context, owner: String, empty: Boolean) {
        val n = if (empty) minOf(count(context, owner) + 1, 12) else 0
        val wait = if (n < 3) 0 else minOf(300_000L, 30_000L * (1L shl minOf(n - 3, 4)))
        prefs(context).edit().putInt("count:$owner", n).putLong("until:$owner", System.currentTimeMillis() + wait).commit()
    }
}
