package com.angelkang.memhot.capture

import java.security.MessageDigest

data class DeviceConfig(val baseUrl: String = "https://memhot.angelkang.com", val token: String = "", val userId: String = "") {
    fun owner(): String = MessageDigest.getInstance("SHA-256")
        .digest("$baseUrl\u0000$userId".toByteArray()).joinToString("") { "%02x".format(it) }
}
