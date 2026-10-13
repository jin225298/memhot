package com.angelkang.memhot.updater

import java.io.File
import org.junit.Assert.*
import org.junit.Test

class UpdatePolicyTest {
    private fun release() = UpdateRelease(1, "com.angelkang.memhot", "0.3.3", 1006, 24,
        "arm64-v8a", "https://memhot.angelkang.com/downloads/memhot-0.3.3-arm64.apk", 3,
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad", listOf("更新"), "2026-10-12T00:00:00Z")

    private fun rejects(block: () -> Unit) {
        try { block(); fail("Unsafe update accepted") } catch (_: IllegalArgumentException) { }
    }

    @Test fun acceptsBoundReleaseAndRejectsUntrustedLocations() {
        release().validate(36)
        release().copy(publishedAt = "2026-10-12T00:00:00.123456+00:00").validate(36)
        listOf("http://memhot.angelkang.com/downloads/memhot-0.3.3-arm64.apk",
            "https://evil.example/downloads/memhot-0.3.3-arm64.apk",
            "https://memhot.angelkang.com.evil.example/downloads/memhot-0.3.3-arm64.apk",
            "https://memhot.angelkang.com/downloads/../memhot-0.3.3-arm64.apk",
            "https://memhot.angelkang.com/downloads/memhot-0.3.3-arm64.apk?token=a",
            "https://memhot.angelkang.com/downloads/memhot-0.3.2-arm64.apk").forEach { url ->
            rejects { release().copy(url = url).validate(36) }
        }
    }
    @Test fun rejectsWrongSchemaPackageAbiAndUnsafeSizes() {
        rejects { release().copy(schemaVersion = 2).validate(36) }
        rejects { release().copy(packageName = "com.evil.app").validate(36) }
        rejects { release().copy(abi = "x86_64").validate(36) }
        rejects { release().copy(size = 0).validate(36) }
        rejects { release().copy(size = 209715201).validate(36) }
        rejects { release().copy(sha256 = "bad").validate(36) }
        rejects { release().copy(minSdk = 37).validate(36) }
        rejects { release().copy(publishedAt = "2026-02-30T00:00:00Z").validate(36) }
        rejects { release().copy(version = "../0.3.3").validate(36) }
        rejects { release().copy(notes = listOf("x".repeat(501))).validate(36) }
    }
    @Test fun artifactDigestRejectsWrongHashAndSize() {
        val file = File.createTempFile("update-policy", ".apk")
        try {
            file.writeText("abc")
            UpdatePolicy.verifyBytes(file, release())
            rejects { UpdatePolicy.verifyBytes(file, release().copy(sha256 = "0".repeat(64))) }
            rejects { UpdatePolicy.verifyBytes(file, release().copy(size = 4)) }
        } finally { file.delete() }
    }
    @Test fun identityRejectsDifferentPackageVersionSignerAndDowngrade() {
        val installed = ApkIdentity("com.angelkang.memhot", "0.3.2", 1005, setOf("installed-signer"))
        val archive = ApkIdentity("com.angelkang.memhot", "0.3.3", 1006, setOf("installed-signer"))
        UpdatePolicy.verifyIdentity(installed, archive, release())
        rejects { UpdatePolicy.verifyIdentity(installed, archive.copy(packageName = "com.evil.app"), release()) }
        rejects { UpdatePolicy.verifyIdentity(installed, archive.copy(versionName = "other"), release()) }
        rejects { UpdatePolicy.verifyIdentity(installed, archive.copy(versionCode = 1007), release()) }
        rejects { UpdatePolicy.verifyIdentity(installed, archive.copy(signers = setOf("other-signer")), release()) }
        rejects { UpdatePolicy.verifyIdentity(installed, archive.copy(signers = emptySet()), release()) }
        rejects { UpdatePolicy.verifyIdentity(installed.copy(versionCode = 1006), archive, release()) }
    }
}
