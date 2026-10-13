package com.angelkang.memhot.updater

import android.app.Activity
import android.os.Handler
import android.os.Looper
import app.tauri.annotation.Command
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import java.util.concurrent.ArrayBlockingQueue
import java.util.concurrent.RejectedExecutionException
import java.util.concurrent.ThreadPoolExecutor
import java.util.concurrent.TimeUnit

@TauriPlugin
class MemhotUpdaterPlugin(private val activity: Activity) : Plugin(activity) {
    private val main = Handler(Looper.getMainLooper())
    @Volatile private var visible = true

    override fun onResume() {
        visible = true
        try { worker.execute { instance(activity).returnedToApp() } } catch (_: Exception) { }
    }
    override fun onPause() { visible = false }
    override fun onStop() { visible = false }

    @Command fun getStatus(invoke: Invoke) = run(invoke) { getStatus() }
    @Command fun checkUpdate(invoke: Invoke) = run(invoke) { checkUpdate() }
    @Command fun downloadUpdate(invoke: Invoke) = run(invoke) { downloadUpdate() }
    @Command fun cancelDownload(invoke: Invoke) = run(invoke) { cancelDownload() }

    @Command fun installUpdate(invoke: Invoke) {
        if (!visible) { invoke.reject("请在应用界面中安装更新"); return }
        enqueue(invoke) {
            try {
                val engine = instance(activity)
                try {
                    val intent = engine.prepareInstall()
                    val state = engine.snapshot()
                    main.post {
                        try {
                            check(visible && !activity.isFinishing && !activity.isDestroyed)
                            activity.startActivity(intent)
                            invoke.resolve(state)
                        } catch (_: Exception) {
                            run(invoke) { fail("无法打开系统安装界面，请返回应用后重试") }
                        }
                    }
                } catch (failure: Exception) {
                    invoke.resolve(engine.fail(UpdateEngine.safeMessage(failure, "无法验证更新包，请重新下载")))
                }
            } catch (_: Exception) { invoke.reject("更新功能暂不可用，请关闭应用后重新打开") }
        }
    }

    private fun run(invoke: Invoke, action: UpdateEngine.() -> JSObject) = enqueue(invoke) {
        try {
            val engine = instance(activity)
            try { invoke.resolve(engine.action()) }
            catch (failure: Exception) {
                invoke.resolve(engine.fail(UpdateEngine.safeMessage(failure, "更新操作失败，请检查网络和存储空间后重试")))
            }
        } catch (_: Exception) { invoke.reject("更新功能暂不可用，请关闭应用后重新打开") }
    }

    private fun enqueue(invoke: Invoke, action: () -> Unit) {
        try { worker.execute(action) }
        catch (_: RejectedExecutionException) { invoke.reject("更新操作正在处理中，请稍后重试") }
    }

    companion object {
        private val worker = ThreadPoolExecutor(1, 1, 0, TimeUnit.SECONDS,
            ArrayBlockingQueue(16), { runnable -> Thread(runnable, "memhot-updater").apply { isDaemon = true } },
            ThreadPoolExecutor.AbortPolicy())
        private var engine: UpdateEngine? = null
        // Accessed only on worker; the application context never retains an Activity/WebView.
        private fun instance(activity: Activity): UpdateEngine = engine ?: UpdateEngine(activity.applicationContext).also { engine = it }
    }
}
