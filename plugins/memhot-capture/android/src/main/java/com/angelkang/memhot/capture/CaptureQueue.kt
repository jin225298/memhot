package com.angelkang.memhot.capture

import android.content.ContentValues
import android.content.Context
import android.database.sqlite.SQLiteDatabase
import android.database.sqlite.SQLiteOpenHelper
import java.io.File
import java.io.FileOutputStream
import java.security.MessageDigest
import java.util.UUID

data class QueuedAudio(val id: String, val file: File, val capturedAt: Long, val attempts: Int,
    val owner: String, val sha256: String, val mode: String = "cloud", val text: String? = null,
    val durationMs: Long = 0)

/** Append files before their DB row; orphan WAVs are recovered on next open. */
class CaptureQueue(context: Context) : SQLiteOpenHelper(context, "memhot-capture.db", null, 2) {
    private val root = File(context.noBackupFilesDir, "audio-queue").apply { mkdirs() }
    override fun onCreate(db: SQLiteDatabase) {
        db.execSQL("CREATE TABLE audio(id TEXT PRIMARY KEY, path TEXT NOT NULL, captured INTEGER NOT NULL, bytes INTEGER NOT NULL, sha256 TEXT NOT NULL, owner TEXT NOT NULL, attempts INTEGER NOT NULL DEFAULT 0, next_at INTEGER NOT NULL DEFAULT 0, acknowledged INTEGER NOT NULL DEFAULT 0, mode TEXT NOT NULL DEFAULT 'cloud', transcript TEXT, duration INTEGER NOT NULL DEFAULT 0, blocked INTEGER NOT NULL DEFAULT 0, retry_key TEXT)")
        db.execSQL("CREATE INDEX audio_due ON audio(owner,next_at,captured)")
    }
    override fun onUpgrade(db: SQLiteDatabase, old: Int, new: Int) {
        if (old < 2) {
            db.execSQL("ALTER TABLE audio ADD COLUMN mode TEXT NOT NULL DEFAULT 'cloud'")
            db.execSQL("ALTER TABLE audio ADD COLUMN transcript TEXT")
            db.execSQL("ALTER TABLE audio ADD COLUMN duration INTEGER NOT NULL DEFAULT 0")
            db.execSQL("ALTER TABLE audio ADD COLUMN blocked INTEGER NOT NULL DEFAULT 0")
            db.execSQL("ALTER TABLE audio ADD COLUMN retry_key TEXT")
            db.execSQL("UPDATE audio SET duration=(bytes-44)/32")
        }
    }
    override fun onConfigure(db: SQLiteDatabase) {
        db.rawQuery("PRAGMA busy_timeout=5000", null).use { it.moveToFirst() }
        db.execSQL("PRAGMA synchronous=FULL")
    }

    fun recover(): Unit = synchronized(fileLock) {
        writableDatabase // Ensure migration precedes reads.
        readableDatabase.rawQuery("SELECT path FROM audio WHERE transcript IS NOT NULL", null).use { rows ->
            while (rows.moveToNext()) {
                val file = File(root, rows.getString(0))
                check(!file.exists() || file.delete()) { "无法清理已转写的本地音频" }
            }
        }
        readableDatabase.rawQuery("SELECT id,path FROM audio WHERE acknowledged=1", null).use { rows ->
            while (rows.moveToNext()) {
                val file = File(root, rows.getString(1))
                if (!file.exists() || file.delete()) writableDatabase.delete("audio", "id=?", arrayOf(rows.getString(0)))
            }
        }
        root.listFiles()?.filter { it.extension == "part" }?.forEach { part ->
            if (part.length() == 0L) part.delete() else {
                val bytes = part.readBytes()
                if (bytes.size in 45..960_044 && bytes.size >= 44) {
                    val declared = java.nio.ByteBuffer.wrap(bytes, 40, 4).order(java.nio.ByteOrder.LITTLE_ENDIAN).int
                    if (declared == bytes.size - 44 && String(bytes.copyOfRange(0, 4)) == "RIFF") {
                        check(part.renameTo(File(root, part.name.removeSuffix(".part")))) { "无法恢复录音片段" }
                    }
                }
            }
        }
        root.listFiles()?.filter { it.extension == "wav" }?.forEach { file ->
            val fields = file.nameWithoutExtension.split('_')
            if (fields.size in 3..4 && fields[1].toLongOrNull() != null && fields[2].length == 64) {
                readableDatabase.rawQuery("SELECT 1 FROM audio WHERE id=?", arrayOf(fields[0])).use {
                    if (!it.moveToFirst()) insert(fields[0], file, fields[1].toLong(), fields[2],
                        if (fields.getOrNull(3) == "local") "local" else "cloud")
                }
            }
        }
    }

    fun append(pcm: ByteArray, capturedAt: Long, owner: String, mode: String = "cloud"): Unit = synchronized(fileLock) {
        require(mode == "local" || mode == "cloud")
        val wav = PcmWav.encode(pcm)
        val files = root.listFiles()?.toList() ?: emptyList()
        check(QueuePolicy.canAppend(maxOf(count(), files.size), files.sumOf { it.length() }, wav.size.toLong(), root.usableSpace)) {
            "录音队列或存储空间已满，请联网上传后重新开始"
        }
        val id = UUID.randomUUID().toString()
        val file = File(root, "${id}_${capturedAt}_${owner}_${mode}.wav")
        val pending = File(root, "${file.name}.part")
        FileOutputStream(pending).use { it.write(wav); it.fd.sync() }
        check(pending.renameTo(file)) { "无法提交录音文件，录音已停止" }
        insert(id, file, capturedAt, owner, mode)
    }

    private fun insert(id: String, file: File, at: Long, owner: String, mode: String = "cloud") {
        val hash = MessageDigest.getInstance("SHA-256").digest(file.readBytes())
            .joinToString("") { "%02x".format(it) }
        val row = ContentValues().apply {
            put("id", id); put("path", file.name); put("captured", at)
            put("bytes", file.length()); put("sha256", hash); put("owner", owner)
            put("mode", mode); put("duration", (file.length() - 44) / 32)
        }
        // Recovery after process death may revisit the same durable file; insertion is idempotent.
        if (writableDatabase.insertWithOnConflict("audio", null, row, SQLiteDatabase.CONFLICT_IGNORE) < 0) {
            readableDatabase.rawQuery("SELECT 1 FROM audio WHERE id=?", arrayOf(id)).use {
                check(it.moveToFirst()) { "无法提交录音队列，录音已停止" }
            }
        }
    }

    @Synchronized fun next(owner: String): QueuedAudio? = readableDatabase.rawQuery(
        "SELECT id,path,captured,attempts,owner,sha256,mode,transcript,duration FROM audio WHERE owner=? AND acknowledged=0 AND blocked=0 AND next_at<=? ORDER BY CASE WHEN mode='local' AND transcript IS NULL THEN 0 ELSE 1 END,captured LIMIT 1",
        arrayOf(owner, System.currentTimeMillis().toString())
    ).use { if (!it.moveToFirst()) null else QueuedAudio(it.getString(0), File(root, it.getString(1)), it.getLong(2), it.getInt(3), it.getString(4), it.getString(5), it.getString(6), if (it.isNull(7)) null else it.getString(7), it.getLong(8)) }

    fun transcribed(item: QueuedAudio, text: String): Unit = synchronized(fileLock) {
        require(item.mode == "local")
        writableDatabase.execSQL("UPDATE audio SET transcript=?,bytes=0 WHERE id=? AND owner=?", arrayOf(text, item.id, item.owner))
        // Durable text is now the source of truth; cloud retry does not need raw audio.
        check(!item.file.exists() || item.file.delete()) { "无法删除已转写的本地音频" }
    }
    @Synchronized fun block(item: QueuedAudio) {
        writableDatabase.execSQL("UPDATE audio SET blocked=1 WHERE id=? AND owner=?", arrayOf(item.id, item.owner))
    }
    @Synchronized fun unblock(owner: String, acceptDuplicate: Boolean) {
        if (!acceptDuplicate && blocked(owner) > 0) error("上次转写结果不明，重试可能再次收费，请明确确认")
        if (acceptDuplicate) writableDatabase.execSQL("UPDATE audio SET retry_key=? WHERE owner=? AND blocked=1", arrayOf(UUID.randomUUID().toString(), owner))
        writableDatabase.execSQL("UPDATE audio SET blocked=0,next_at=0 WHERE owner=?", arrayOf(owner))
    }
    @Synchronized fun retryKey(item: QueuedAudio): String? = readableDatabase.rawQuery(
        "SELECT retry_key FROM audio WHERE id=? AND owner=?", arrayOf(item.id, item.owner)
    ).use { if (it.moveToFirst() && !it.isNull(0)) it.getString(0) else null }
    @Synchronized fun blocked(owner: String): Int = readableDatabase.rawQuery(
        "SELECT COUNT(*) FROM audio WHERE owner=? AND blocked=1", arrayOf(owner)
    ).use { it.moveToFirst(); it.getInt(0) }

    fun acknowledged(item: QueuedAudio): Unit = synchronized(fileLock) {
        // Commit receipt first. Crash recovery finishes cleanup without reuploading accepted audio.
        writableDatabase.execSQL("UPDATE audio SET acknowledged=1 WHERE id=?", arrayOf(item.id))
        check(!item.file.exists() || item.file.delete()) { "无法清理已上传音频" }
        writableDatabase.delete("audio", "id=?", arrayOf(item.id))
    }

    @Synchronized fun retry(item: QueuedAudio) {
        val delay = minOf(60 * 60_000L, 30_000L * (1L shl minOf(item.attempts, 7)))
        val row = ContentValues().apply {
            put("attempts", item.attempts + 1); put("next_at", System.currentTimeMillis() + delay)
        }
        writableDatabase.update("audio", row, "id=?", arrayOf(item.id))
    }

    @Synchronized fun count(): Int = readableDatabase.rawQuery("SELECT COUNT(*) FROM audio", null).use { it.moveToFirst(); it.getInt(0) }
    @Synchronized fun pending(owner: String): Int = readableDatabase.rawQuery("SELECT COUNT(*) FROM audio WHERE owner=? AND acknowledged=0 AND blocked=0", arrayOf(owner)).use { it.moveToFirst(); it.getInt(0) }
    @Synchronized fun needsLocalDecode(owner: String): Boolean = readableDatabase.rawQuery("SELECT EXISTS(SELECT 1 FROM audio WHERE owner=? AND mode='local' AND transcript IS NULL AND acknowledged=0 AND blocked=0)", arrayOf(owner)).use { it.moveToFirst(); it.getInt(0) != 0 }
    @Synchronized fun nextDelay(owner: String): Long = readableDatabase.rawQuery(
        "SELECT COALESCE(MIN(next_at),0) FROM audio WHERE owner=? AND acknowledged=0 AND blocked=0", arrayOf(owner)
    ).use { it.moveToFirst(); maxOf(60_000, it.getLong(0) - System.currentTimeMillis()) }

    fun clearOwner(owner: String): Int = synchronized(fileLock) {
        val count = pending(owner) + blocked(owner)
        root.listFiles()?.filter { it.name.contains("_${owner}.") || it.name.contains("_${owner}_") }
            ?.forEach { check(it.delete()) { "无法删除本地录音片段" } }
        writableDatabase.delete("audio", "owner=?", arrayOf(owner))
        count
    }

    companion object {
        // Service, uploader and UI create separate SQLiteOpenHelpers. Their instance
        // monitors cannot protect the shared .part -> .wav commit from recovery/deletion.
        private val fileLock = Any()
    }
}
