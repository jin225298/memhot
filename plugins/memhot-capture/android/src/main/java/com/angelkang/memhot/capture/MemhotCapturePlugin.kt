package com.angelkang.memhot.capture

import android.Manifest
import android.app.Activity
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.provider.Settings
import app.tauri.annotation.Command
import app.tauri.annotation.Permission
import app.tauri.annotation.PermissionCallback
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin

@TauriPlugin(permissions = [
    Permission(strings = [Manifest.permission.RECORD_AUDIO], alias = "microphone"),
    Permission(strings = [Manifest.permission.POST_NOTIFICATIONS], alias = "notifications")
])
class MemhotCapturePlugin(private val activity: Activity) : Plugin(activity) {
    @Volatile private var visible = true
    private var stopped = false
    @Volatile private var launchTarget: String? = activity.intent?.getStringExtra("memhotLaunchTarget")
    private val main = Handler(Looper.getMainLooper())
    private var permissionResume: Invoke? = null
    private val permissionTimeout = Runnable { cancelPermissionStart("授权后未返回应用，请重新点击麦克风") }
    override fun onResume() {
        visible = true
        stopped = false
        takePermissionStart()?.let { start(it) }
    }
    override fun onPause() { visible = false }
    override fun onStop() {
        visible = false
        stopped = true
        cancelPermissionStart("录音启动已取消，请回到应用后重新点击麦克风")
    }

    private fun takePermissionStart(): Invoke? {
        main.removeCallbacks(permissionTimeout)
        return permissionResume.also { permissionResume = null }
    }

    private fun cancelPermissionStart(message: String) { takePermissionStart()?.reject(message) }

    private fun continueAfterPermission(invoke: Invoke) {
        // Permission results can arrive between onPause and onResume. Starting a
        // microphone service there either rejects our visibility check or races
        // Android's while-in-use permission gate. Resume only this explicit request.
        if (stopped) { invoke.reject("录音启动已取消，请回到应用后重新点击麦克风"); return }
        if (visible) { start(invoke); return }
        if (permissionResume != null) { invoke.reject("正在等待授权完成，请稍候"); return }
        permissionResume = invoke
        main.postDelayed(permissionTimeout, 5000)
    }
    override fun onNewIntent(intent: Intent) {
        if (intent.getStringExtra("memhotLaunchTarget") == "today") launchTarget = "today"
    }
    @Command fun getLaunchTarget(invoke: Invoke) {
        val target = launchTarget
        launchTarget = null
        activity.intent?.removeExtra("memhotLaunchTarget")
        invoke.resolve(JSObject().put("tab", if (target == "today") "today" else org.json.JSONObject.NULL))
    }

    @Command fun configure(invoke: Invoke) {
        cancelPermissionStart("账户已切换，请重新点击麦克风")
        val args = invoke.getArgs()
        val stopped = CaptureService.requestStopNow()
        activity.stopService(Intent(activity, CaptureService::class.java))
        Thread({
            try {
                // Finish the old recorder before replacing its account configuration.
                stopped?.join(5000)
                check(stopped?.isAlive != true && !CaptureState.recording)
                val token = args.getString("token") ?: ""
                SecureConfig(activity).write(args.getString("baseUrl") ?: "https://memhot.angelkang.com", token, args.getString("userId") ?: "")
                CaptureState.clearAuthenticationError(activity)
                DigestSyncWorker.configure(activity, token.isNotEmpty())
                // Stable user identity + URL lets refreshed credentials resume the same queue.
                if (token.isNotEmpty()) UploadDispatcher.schedule(activity)
                invoke.resolve(JSObject())
            } catch (_: Exception) { invoke.reject("无法保存登录信息，请确认录音已停止并检查服务器地址") }
        }, "memhot-configure").start()
    }

    @Command fun getConfig(invoke: Invoke) {
        try {
            val config = SecureConfig(activity).read()
            if (config.token.isNotEmpty()) UploadDispatcher.schedule(activity)
            DigestSyncWorker.configure(activity, config.token.isNotEmpty())
            invoke.resolve(JSObject().put("baseUrl", config.baseUrl).put("token", config.token).put("userId", config.userId))
        } catch (_: Exception) { invoke.reject("无法读取登录信息，请重新登录") }
    }

    @Command fun start(invoke: Invoke) {
        if (!visible) { invoke.reject("请在应用界面中开始录音"); return }
        if (activity.checkSelfPermission(Manifest.permission.RECORD_AUDIO) != PackageManager.PERMISSION_GRANTED) {
            requestPermissionForAlias("microphone", invoke, "microphonePermission")
            return
        }
        if (Build.VERSION.SDK_INT >= 33 && activity.checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED) {
            requestPermissionForAlias("notifications", invoke, "notificationPermission")
            return
        }
        begin(invoke)
    }

    @PermissionCallback fun microphonePermission(invoke: Invoke) {
        if (activity.checkSelfPermission(Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED) continueAfterPermission(invoke)
        else invoke.reject("麦克风权限被拒绝，未开始录音")
    }

    @PermissionCallback fun notificationPermission(invoke: Invoke) {
        if (Build.VERSION.SDK_INT < 33 || activity.checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED) continueAfterPermission(invoke)
        else invoke.reject("请允许录音通知，以便随时从通知栏停止录音")
    }

    private fun begin(invoke: Invoke) {
        try {
            check(visible) { "Activity not visible" }
            check(SecureConfig(activity).read().token.isNotEmpty()) { "Login required" }
            if (CaptureState.recording) { invoke.resolve(CaptureState.snapshot(activity)); return }
            CaptureState.set(activity, "starting")
            val intent = Intent(activity, CaptureService::class.java)
            if (Build.VERSION.SDK_INT >= 26) activity.startForegroundService(intent) else activity.startService(intent)
            val deadline = SystemClock.elapsedRealtime() + 3000
            // Resolve only after AudioRecord starts or the service reports a concrete failure.
            val poll = object : Runnable {
                override fun run() {
                    try {
                        val state = CaptureState.snapshot(activity)
                        if (CaptureState.recording || state.getString("state") in listOf("paused", "error")) {
                            invoke.resolve(state)
                        } else if (SystemClock.elapsedRealtime() >= deadline) {
                            CaptureService.requestStopNow()
                            activity.stopService(intent)
                            invoke.reject("麦克风未能在3秒内启动，请检查系统权限及后台限制后重试")
                        } else main.postDelayed(this, 50)
                    } catch (_: Exception) {
                        CaptureService.requestStopNow()
                        activity.stopService(intent)
                        invoke.reject("无法确认录音状态，请检查设备存储空间后重试")
                    }
                }
            }
            main.post(poll)
        } catch (_: Exception) {
            CaptureState.set(activity, "error", "无法开始录音，请检查登录状态、麦克风权限及系统后台限制")
            invoke.reject("无法开始录音，请检查登录状态、麦克风权限及系统后台限制")
        }
    }

    @Command fun stop(invoke: Invoke) {
        cancelPermissionStart("录音启动已取消")
        val thread = CaptureService.requestStopNow()
        activity.stopService(Intent(activity, CaptureService::class.java))
        Thread({
            try {
                thread?.join(5000)
                check(thread?.isAlive != true && !CaptureState.recording)
                val state = CaptureState.snapshot(activity)
                if (state.getString("state") == "starting") CaptureState.set(activity, "idle")
                invoke.resolve(CaptureState.snapshot(activity))
            } catch (_: Exception) { invoke.reject("录音尚未完全停止，请稍后重试；已提交片段仍在本地队列中") }
        }, "memhot-stop-recording").start()
    }

    @Command fun clearPending(invoke: Invoke) {
        UploadDispatcher.clearPending(activity.applicationContext) { count, error ->
            if (error == null) invoke.resolve(JSObject().put("clearedCount", count))
            else invoke.reject(error)
        }
    }

    @Command fun setAsrMode(invoke: Invoke) {
        try {
            LocalModel.setMode(activity, invoke.getArgs().getString("mode") ?: "")
            invoke.resolve(CaptureState.snapshot(activity))
        } catch (error: Exception) { invoke.reject(error.message ?: "无法切换转写方式") }
    }
    @Command fun downloadLocalModel(invoke: Invoke) {
        LocalModel.download(activity.applicationContext)
        invoke.resolve(LocalModel.status(activity))
    }
    @Command fun cancelLocalModelDownload(invoke: Invoke) {
        LocalModel.cancel()
        invoke.resolve(LocalModel.status(activity))
    }
    @Command fun removeLocalModel(invoke: Invoke) {
        Thread({
            try { LocalModel.remove(activity); invoke.resolve(LocalModel.status(activity)) }
            catch (error: Exception) { invoke.reject(error.message ?: "无法删除模型") }
        }, "memhot-remove-model").start()
    }
    @Command fun retryPending(invoke: Invoke) {
        try {
            val config = SecureConfig(activity).read()
            CaptureQueue(activity).use { it.unblock(config.owner(), invoke.getArgs().optBoolean("acceptDuplicateCharge", false)) }
            CaptureState.clearSyncError(activity, config.owner())
            UploadDispatcher.schedule(activity, 1000)
            invoke.resolve(CaptureState.snapshot(activity))
        } catch (error: Exception) { invoke.reject(error.message ?: "无法恢复录音队列") }
    }

    @Command fun requestNotifications(invoke: Invoke) {
        if (!visible) { invoke.reject("请在应用界面中开启提醒"); return }
        if (Build.VERSION.SDK_INT >= 33 && activity.checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED) {
            requestPermissionForAlias("notifications", invoke, "digestNotificationPermission")
        } else if (!DigestSyncWorker.notificationsAllowed(activity)) {
            openNotificationSettings()
            invoke.resolve(CaptureState.snapshot(activity))
        } else invoke.resolve(CaptureState.snapshot(activity))
    }

    @PermissionCallback fun digestNotificationPermission(invoke: Invoke) {
        if (Build.VERSION.SDK_INT >= 33 && !DigestSyncWorker.notificationsAllowed(activity) &&
            !activity.shouldShowRequestPermissionRationale(Manifest.permission.POST_NOTIFICATIONS)) openNotificationSettings()
        invoke.resolve(CaptureState.snapshot(activity))
    }

    private fun openNotificationSettings() {
        val intent = if (Build.VERSION.SDK_INT >= 26) Intent(Settings.ACTION_APP_NOTIFICATION_SETTINGS)
            .putExtra(Settings.EXTRA_APP_PACKAGE, activity.packageName)
        else Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, android.net.Uri.parse("package:${activity.packageName}"))
        activity.startActivity(intent)
    }

    @Command fun status(invoke: Invoke) {
        try { invoke.resolve(CaptureState.snapshot(activity)) }
        catch (_: Exception) { invoke.reject("无法读取录音队列，请检查存储空间") }
    }

    @Command fun openBatterySettings(invoke: Invoke) {
        try {
            activity.startActivity(Intent(Settings.ACTION_IGNORE_BATTERY_OPTIMIZATION_SETTINGS))
            invoke.resolve(JSObject())
        } catch (_: Exception) { invoke.reject("请手动打开设置 → 电池 → Memhot，允许后台活动") }
    }
}
