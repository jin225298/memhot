package com.angelkang.memhot;

import android.Manifest;
import android.app.Activity;
import android.app.Instrumentation;
import android.content.Intent;
import android.graphics.Insets;
import android.os.Build;
import android.os.Bundle;
import android.os.SystemClock;
import android.view.View;
import android.view.ViewGroup;
import android.view.WindowInsets;
import android.webkit.WebView;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;

/** SDK-only runner tests the exact shrunk APK without depending on shared
 * Kotlin/AndroidX symbols that release R8 may inline or remove. Silent emulator
 * only; configuration is kept in WebView memory and restored without logging it.
 */
public class NavigationSmokeTest extends Instrumentation {
    interface Condition { boolean get() throws Exception; }
    @Override public void onCreate(Bundle arguments) { super.onCreate(arguments); start(); }
    @Override public void onStart() {
        try {
            require(Build.MODEL.indexOf("sdk_gphone") >= 0 && Build.VERSION.SDK_INT >= 33, "Emulator only");
            getUiAutomation().grantRuntimePermission(getTargetContext().getPackageName(), Manifest.permission.RECORD_AUDIO);
            getUiAutomation().grantRuntimePermission(getTargetContext().getPackageName(), Manifest.permission.POST_NOTIFICATIONS);
            Intent intent = new Intent().setClassName(getTargetContext().getPackageName(), "com.angelkang.memhot.MainActivity")
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
            Activity activity = startActivitySync(intent);
            WebView[] pending = new WebView[1];
            await("WebView did not initialize", () -> {
                runOnMainSync(() -> pending[0] = findWebView(activity.findViewById(android.R.id.content)));
                return pending[0] != null;
            });
            WebView web = pending[0];
            await("Tauri bridge did not initialize", () -> js(web, "typeof window.__TAURI_INTERNALS__?.invoke === 'function'").equals("true"));
            command(web, "get_config", "undefined", "window.__previousCaptureConfig=result", false);
            try {
                verifyInsets(activity);
                command(web, "configure", "{baseUrl:'https://navigation-test.invalid',token:'',userId:''}", "", false);
                String rejection = command(web, "start", "undefined", "", true);
                require(rejection.indexOf("无法开始录音") >= 0, "Native error was lost: " + rejection);
                command(web, "configure", "{baseUrl:'https://navigation-test.invalid',token:'navigation-only-fake-token-00000000000000000000',userId:'navigation-test-user'}", "", false);
                command(web, "start");
                require(command(web, "status").indexOf("recording") >= 0, "Native start did not record");
                js(web, "window.__backDepth=2;window.addEventListener('memhot:navigate-back',e=>{if(window.__backDepth>0){window.__backDepth--;e.preventDefault();}e.stopImmediatePropagation();},true);true");
                for (int depth = 1; depth >= 0; depth--) {
                    final String expected = Integer.toString(depth);
                    runOnMainSync(activity::onBackPressed);
                    await("Back did not leave exactly one page", () -> js(web, "window.__backDepth").equals(expected));
                    require(activity.hasWindowFocus() && !activity.isFinishing(), "Nested Back backgrounded app");
                }
                runOnMainSync(activity::onBackPressed);
                await("Root Back did not background task", () -> !activity.hasWindowFocus());
                SystemClock.sleep(1200);
                require(!activity.isDestroyed() && !activity.isFinishing(), "Root Back destroyed Activity");
                getTargetContext().startActivity(intent.addFlags(Intent.FLAG_ACTIVITY_REORDER_TO_FRONT));
                await("Activity did not return", activity::hasWindowFocus);
                require(command(web, "status").indexOf("recording") >= 0, "Background stopped recording");
                command(web, "stop");
                require(command(web, "status").indexOf("idle") >= 0, "Stop did not finish");
            } finally {
                getTargetContext().startActivity(intent.addFlags(Intent.FLAG_ACTIVITY_REORDER_TO_FRONT));
                await("Activity did not return for cleanup", activity::hasWindowFocus);
                command(web, "stop");
                command(web, "clear_pending");
                command(web, "configure", "window.__previousCaptureConfig", "", false);
            }
            Bundle result = new Bundle();
            result.putString("stream", "\nPASS release bridge rejection/start/stop, nested Back, background recording, system/IME insets\n");
            finish(Activity.RESULT_OK, result);
        } catch (Throwable error) {
            Bundle result = new Bundle();
            result.putString("stream", "\nFAIL " + error.getClass().getSimpleName() + ": " + error.getMessage() + "\n");
            finish(Activity.RESULT_CANCELED, result);
        }
    }
    private void verifyInsets(Activity activity) {
        Throwable[] problem = new Throwable[1];
        runOnMainSync(() -> {
            try {
            View root = activity.findViewById(android.R.id.content);
            Insets bars = root.getRootWindowInsets().getInsets(WindowInsets.Type.systemBars());
            require(root.getPaddingTop() >= bars.top && root.getPaddingTop() > 0, "Status bar overlaps content");
            WindowInsets keyboard = new WindowInsets.Builder().setInsets(WindowInsets.Type.systemBars(), Insets.of(0, 30, 0, 24))
                .setInsets(WindowInsets.Type.ime(), Insets.of(0, 0, 0, 300)).build();
            root.dispatchApplyWindowInsets(keyboard);
            require(root.getPaddingBottom() == 300, "Keyboard insets not applied: " + root.getPaddingBottom());
            root.dispatchApplyWindowInsets(new WindowInsets.Builder(keyboard).setInsets(WindowInsets.Type.ime(), Insets.NONE).build());
            require(root.getPaddingBottom() == 24, "Keyboard left ghost padding");
            root.requestApplyInsets();
            } catch (Throwable error) { problem[0] = error; }
        });
        if (problem[0] != null) throw new AssertionError(problem[0].getMessage());
    }
    private String command(WebView web, String name) throws Exception { return command(web, name, "undefined", "", false); }
    private String command(WebView web, String name, String args, String then, boolean rejectExpected) throws Exception {
        js(web, "window.__nativeResult=null;window.__TAURI_INTERNALS__.invoke('plugin:memhot-capture|" + name + "'," + args + ").then(result=>{" + then + ";window.__nativeResult={ok:true,result};}).catch(e=>window.__nativeResult={ok:false,error:String(e)});true");
        await("Native " + name + " did not complete", () -> js(web, "window.__nativeResult!==null").equals("true"));
        if (!js(web, "window.__nativeResult.ok").equals(rejectExpected ? "false" : "true"))
            throw new AssertionError("Native " + name + ": " + js(web, "window.__nativeResult.error || 'resolved'"));
        return name.equals("get_config") ? "saved" : js(web, "window.__nativeResult");
    }
    private WebView findWebView(View view) {
        if (view instanceof WebView) return (WebView) view;
        if (view instanceof ViewGroup) {
            ViewGroup group = (ViewGroup) view;
            for (int i = 0; i < group.getChildCount(); i++) {
                WebView found = findWebView(group.getChildAt(i));
                if (found != null) return found;
            }
        }
        return null;
    }
    private String js(WebView web, String script) throws Exception {
        CountDownLatch ready = new CountDownLatch(1);
        String[] result = new String[1];
        runOnMainSync(() -> web.evaluateJavascript(script, value -> { result[0] = value; ready.countDown(); }));
        require(ready.await(5, TimeUnit.SECONDS), "WebView did not reply");
        return result[0];
    }
    private void await(String message, Condition condition) throws Exception {
        long start = SystemClock.elapsedRealtime();
        while (!condition.get() && SystemClock.elapsedRealtime() - start < 15000) SystemClock.sleep(100);
        require(condition.get(), message);
    }
    private static void require(boolean pass, String message) { if (!pass) throw new AssertionError(message); }
}
