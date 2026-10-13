package com.angelkang.memhot.capture

import android.database.sqlite.SQLiteDatabase
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class QueuePersistenceTest {
    @Test fun recoveryInAnotherInstanceCannotStealAnActiveWriterFile() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val owner = DeviceConfig("https://queue-test.invalid", "token", "concurrent-test").owner()
        val executor = java.util.concurrent.Executors.newFixedThreadPool(2)
        val begin = java.util.concurrent.CountDownLatch(1)
        CaptureQueue(context).use { it.clearOwner(owner) }
        try {
            val writer = executor.submit {
                CaptureQueue(context).use { queue ->
                    begin.await()
                    repeat(40) { queue.append(ByteArray(960_000), 1_800_000_000_000 + it * 30_000L, owner) }
                }
            }
            val recovery = executor.submit {
                CaptureQueue(context).use { queue ->
                    begin.await()
                    repeat(40) { queue.recover() }
                }
            }
            begin.countDown()
            writer.get(60, java.util.concurrent.TimeUnit.SECONDS)
            recovery.get(60, java.util.concurrent.TimeUnit.SECONDS)
            CaptureQueue(context).use { queue ->
                queue.recover()
                assertEquals(40, queue.pending(owner))
                repeat(40) {
                    val audio = queue.next(owner)!!
                    assertEquals(960_044L, audio.file.length())
                    queue.acknowledged(audio)
                }
                assertEquals(0, queue.pending(owner))
            }
        } finally {
            executor.shutdownNow()
            CaptureQueue(context).use { it.clearOwner(owner) }
        }
    }

    @Test fun restartRecoversOrphanAndClearingOneOwnerKeepsAnother() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val alice = DeviceConfig("https://queue-test.invalid", "token-a", "alice-test").owner()
        val bob = DeviceConfig("https://queue-test.invalid", "token-b", "bob-test").owner()
        CaptureQueue(context).use { queue ->
            queue.clearOwner(alice); queue.clearOwner(bob)
            queue.append(ByteArray(32_000), 1_800_000_000_000, alice)
            queue.append(ByteArray(32_000), 1_800_000_001_000, bob)
        }
        val id = CaptureQueue(context).use { queue ->
            val stored = queue.next(alice)!!
            assertEquals(32_044L, stored.file.length())
            assertEquals(64, stored.sha256.length)
            stored.id
        }
        // Simulate a crash after WAV rename and before its SQLite row commit.
        SQLiteDatabase.openDatabase(context.getDatabasePath("memhot-capture.db").path, null, SQLiteDatabase.OPEN_READWRITE).use {
            it.delete("audio", "id=?", arrayOf(id))
        }
        try {
            CaptureQueue(context).use { queue ->
                queue.recover()
                assertEquals(id, queue.next(alice)!!.id)
                assertEquals(1, queue.clearOwner(alice))
                assertEquals(1, queue.pending(bob))
            }
            CaptureQueue(context).use { queue ->
                queue.recover()
                assertNull(queue.next(alice))
                assertEquals(1, queue.pending(bob))
            }
        } finally {
            CaptureQueue(context).use { it.clearOwner(alice); it.clearOwner(bob) }
        }
    }
}
