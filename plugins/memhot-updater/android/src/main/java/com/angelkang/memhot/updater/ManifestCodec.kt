package com.angelkang.memhot.updater

import org.json.JSONArray
import org.json.JSONObject
import android.util.JsonReader
import android.util.JsonToken
import java.io.StringReader

/** Explicit JSON avoids reflective serialization and survives release shrinking. */
object ManifestCodec {
    private val keys = setOf("schemaVersion", "packageName", "version", "versionCode", "minSdk",
        "abi", "url", "size", "sha256", "notes", "publishedAt")

    fun parse(text: String, deviceSdk: Int): UpdateRelease {
        require(text.toByteArray(Charsets.UTF_8).size <= UpdatePolicy.MAX_MANIFEST_BYTES)
        val values = mutableMapOf<String, Any>()
        JsonReader(StringReader(text)).use { reader ->
            reader.isLenient = false
            reader.beginObject()
            while (reader.hasNext()) {
                val key = reader.nextName()
                require(key in keys && key !in values)
                values[key] = when (key) {
                    "schemaVersion", "versionCode", "minSdk", "size" -> {
                        require(reader.peek() == JsonToken.NUMBER)
                        val number = reader.nextString()
                        require(number.matches(Regex("(0|[1-9][0-9]*)")))
                        number.toLongOrNull() ?: throw IllegalArgumentException("Invalid manifest number")
                    }
                    "notes" -> {
                        val notes = mutableListOf<String>()
                        reader.beginArray()
                        while (reader.hasNext()) {
                            require(notes.size < 16 && reader.peek() == JsonToken.STRING)
                            notes.add(reader.nextString().also { require(it.length <= 500) })
                        }
                        reader.endArray()
                        notes
                    }
                    else -> {
                        require(reader.peek() == JsonToken.STRING)
                        reader.nextString()
                    }
                }
            }
            reader.endObject()
            require(reader.peek() == JsonToken.END_DOCUMENT && values.keys == keys)
        }
        fun string(key: String) = values.getValue(key) as String
        fun number(key: String): Long {
            return values.getValue(key) as Long
        }
        fun integer(key: String): Int {
            val value = number(key)
            require(value in 0..Int.MAX_VALUE.toLong())
            return value.toInt()
        }
        @Suppress("UNCHECKED_CAST")
        val notes = values.getValue("notes") as List<String>
        return UpdateRelease(integer("schemaVersion"), string("packageName"), string("version"), number("versionCode"),
            integer("minSdk"), string("abi"), string("url"), number("size"), string("sha256"),
            notes,
            string("publishedAt")).also { it.validate(deviceSdk) }
    }

    fun json(release: UpdateRelease) = JSONObject()
        .put("schemaVersion", release.schemaVersion).put("packageName", release.packageName)
        .put("version", release.version).put("versionCode", release.versionCode).put("minSdk", release.minSdk)
        .put("abi", release.abi).put("url", release.url).put("size", release.size).put("sha256", release.sha256)
        .put("notes", JSONArray(release.notes)).put("publishedAt", release.publishedAt)
}
