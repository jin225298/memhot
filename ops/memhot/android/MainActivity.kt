package com.angelkang.memhot

import android.os.Bundle
import android.webkit.WebView
import android.view.View
import android.view.WindowManager
import androidx.activity.OnBackPressedCallback
import androidx.activity.enableEdgeToEdge
import androidx.core.graphics.Insets
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat
import app.tauri.plugin.PluginManager

class MainActivity : TauriActivity() {
    private var appWebView: WebView? = null
    private var backPending = false
    override fun onCreate(savedInstanceState: Bundle?) {
        // Disable release inspection before creating a WebView. Chromium may
        // override this app policy on a system userdebug build.
        WebView.setWebContentsDebuggingEnabled(BuildConfig.DEBUG)
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)
        window.setSoftInputMode(WindowManager.LayoutParams.SOFT_INPUT_ADJUST_RESIZE)
        val content = findViewById<View>(android.R.id.content)
        ViewCompat.setOnApplyWindowInsetsListener(content) { view, windowInsets ->
            // Keep the whole scrolling surface clear of system bars and the IME.
            // Zero handled types (rather than consume) so WebView receives hide
            // updates too and does not retain keyboard padding or double it.
            val types = WindowInsetsCompat.Type.systemBars() or
                WindowInsetsCompat.Type.displayCutout() or WindowInsetsCompat.Type.ime()
            val insets = windowInsets.getInsets(types)
            view.setPadding(insets.left, insets.top, insets.right, insets.bottom)
            WindowInsetsCompat.Builder(windowInsets).setInsets(types, Insets.NONE).build()
        }
        ViewCompat.requestApplyInsets(content)
        onBackPressedDispatcher.addCallback(this, object : OnBackPressedCallback(true) {
            override fun handleOnBackPressed() = navigateBack()
        })
    }

    override fun onWebViewCreate(webView: WebView) {
        super.onWebViewCreate(webView)
        appWebView = webView
    }

    // Tauri 2.11.5 generates an observer but never registers it. Forward the
    // Activity lifecycle directly: permission dialogs need immediate visibility,
    // not ProcessLifecycleOwner's delayed background events. Recheck on upgrade.
    override fun onResume() { super.onResume(); PluginManager.onResume() }
    override fun onPause() { PluginManager.onPause(); super.onPause() }
    override fun onStop() { PluginManager.onStop(); super.onStop() }

    private fun navigateBack() {
        if (backPending) return
        val webView = appWebView
        if (webView == null) { moveTaskToBack(true); return }
        backPending = true
        // Let the active React screen (or an open menu/editor) consume one Back.
        // Android dismisses the IME before dispatching here. At the root, retain
        // the Activity/process so the microphone foreground service can continue.
        webView.evaluateJavascript(
            "!window.dispatchEvent(new Event('memhot:navigate-back', {cancelable: true}))"
        ) { handled ->
            backPending = false
            if (handled != "true" && !isFinishing && !isDestroyed) moveTaskToBack(true)
        }
    }
}
