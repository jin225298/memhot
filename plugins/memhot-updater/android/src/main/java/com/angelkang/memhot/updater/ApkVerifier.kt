package com.angelkang.memhot.updater

import android.content.Context
import android.content.pm.PackageInfo
import android.content.pm.PackageManager
import android.os.Build
import java.io.File
import java.security.MessageDigest

object ApkVerifier {
    @Suppress("DEPRECATION")
    private fun flags(): Int = if (Build.VERSION.SDK_INT >= 28) PackageManager.GET_SIGNING_CERTIFICATES else PackageManager.GET_SIGNATURES

    @Suppress("DEPRECATION")
    private fun identity(info: PackageInfo): ApkIdentity {
        val signatures = if (Build.VERSION.SDK_INT >= 28) info.signingInfo?.apkContentsSigners else info.signatures
        val signers = signatures.orEmpty().map { UpdatePolicy.hex(MessageDigest.getInstance("SHA-256").digest(it.toByteArray())) }.toSet()
        val code = if (Build.VERSION.SDK_INT >= 28) info.longVersionCode else info.versionCode.toLong()
        return ApkIdentity(info.packageName, info.versionName ?: "", code, signers)
    }
    @Suppress("DEPRECATION")
    fun installed(context: Context) = identity(context.packageManager.getPackageInfo(context.packageName, flags()))

    @Suppress("DEPRECATION")
    fun verify(context: Context, file: File, release: UpdateRelease) {
        release.validate(Build.VERSION.SDK_INT)
        UpdatePolicy.verifyBytes(file, release)
        val archive = context.packageManager.getPackageArchiveInfo(file.absolutePath, flags())
            ?: throw IllegalArgumentException("更新包无法读取，请重新下载")
        UpdatePolicy.verifyIdentity(installed(context), identity(archive), release)
        if (Build.VERSION.SDK_INT >= 24) require(archive.applicationInfo?.minSdkVersion == release.minSdk) { "更新包系统要求不符" }
    }
}
