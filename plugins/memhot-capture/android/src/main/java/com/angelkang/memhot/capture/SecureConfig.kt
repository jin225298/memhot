package com.angelkang.memhot.capture

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import org.json.JSONObject
import java.net.URI
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/** Only encrypted ciphertext enters SharedPreferences; key stays in AndroidKeyStore. */
class SecureConfig(context: Context) {
    private val prefs = context.getSharedPreferences("memhot-secure", Context.MODE_PRIVATE)
    private val alias = "memhot-device-config-v1"

    @Synchronized fun read(): DeviceConfig {
        val value = prefs.getString("encrypted", null) ?: return DeviceConfig()
        val packed = Base64.decode(value, Base64.NO_WRAP)
        require(packed.size > 28) { "登录信息无法读取，请重新登录" }
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(128, packed.copyOfRange(0, 12)))
        val json = JSONObject(String(cipher.doFinal(packed.copyOfRange(12, packed.size)), Charsets.UTF_8))
        return DeviceConfig(json.getString("baseUrl"), json.getString("token"), json.optString("userId", ""))
    }

    @Synchronized fun write(baseUrl: String, token: String, userId: String) {
        val uri = URI(baseUrl)
        require(uri.scheme == "https" && !uri.host.isNullOrBlank() && uri.userInfo == null &&
            uri.query == null && uri.fragment == null) { "服务器地址须使用 HTTPS" }
        require(token.isEmpty() || token.length in 32..256 && token.all { it.code in 33..126 }) {
            "登录信息无效，请重新登录"
        }
        require(token.isEmpty() || userId.isNotBlank() && userId.length <= 128 && userId.none { it.isISOControl() }) {
            "当前账户信息无效，请重新登录"
        }
        val json = JSONObject().put("baseUrl", baseUrl.trimEnd('/')).put("token", token).put("userId", userId).toString()
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.ENCRYPT_MODE, key())
        val encrypted = cipher.iv + cipher.doFinal(json.toByteArray(Charsets.UTF_8))
        check(prefs.edit().putString("encrypted", Base64.encodeToString(encrypted, Base64.NO_WRAP)).commit()) {
            "无法保存登录信息，请重试"
        }
    }

    private fun key(): SecretKey {
        val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        (store.getKey(alias, null) as? SecretKey)?.let { return it }
        return KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore").apply {
            init(KeyGenParameterSpec.Builder(alias, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE).build())
        }.generateKey()
    }
}
