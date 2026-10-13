package com.angelkang.memhot;

import android.app.Activity;
import android.app.Instrumentation;
import android.content.Intent;
import android.os.Build;
import android.os.Bundle;
import android.os.SystemClock;
import android.view.View;
import android.view.ViewGroup;
import android.webkit.WebView;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import org.json.JSONObject;

/** Runs through the production JS/Rust/native bridge on the signed, shrunk app.
 * Use a private same-signer bootstrap with the previous versionCode for download
 * tests. No alternate feed, downgrade flag, or security bypass exists in the app.
 * System installation confirmation is left to the user; this runner never taps it.
 */
public class UpdateSmokeTest extends Instrumentation {
    interface Condition { boolean get() throws Exception; }
    private Bundle arguments;
    @Override public void onCreate(Bundle args) { super.onCreate(args); arguments = args; start(); }
    @Override public void onStart() {
        try {
            require(Build.MODEL.contains("sdk_gphone"), "Emulator only");
            String mode = arguments.getString("mode", "latest");
            Intent launch = new Intent().setClassName(getTargetContext().getPackageName(), "com.angelkang.memhot.MainActivity")
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
            Activity activity = startActivitySync(launch);
            WebView[] pending = new WebView[1];
            await("WebView not ready", 15000, () -> {
                runOnMainSync(() -> pending[0] = find(activity.findViewById(android.R.id.content)));
                return pending[0] != null;
            });
            WebView web = pending[0];
            await("Bridge not ready", 15000, () -> js(web, "typeof window.__TAURI_INTERNALS__?.invoke === 'function'").equals("true"));
            JSONObject state = command(web, "get_status");
            require(state.getBoolean("supported"), "Native updater unsupported");
            require(state.getInt("currentVersionCode") > 0, "Installed version unavailable");
            state = command(web, "check_update");
            if (mode.equals("latest")) {
                require(state.getString("phase").equals("idle") && state.isNull("release"), "Installed version should be current");
                pass("latest signed release reports current version, no update offered");
                return;
            }
            require(state.getJSONObject("release").getInt("versionCode") > state.getInt("currentVersionCode"), "Feed must offer a real upgrade");
            if (mode.equals("download")) {
                state = command(web, "download_update");
                require(state.getString("phase").equals("downloading"), "Download did not start");
                state = command(web, "cancel_download");
                require(!state.getString("phase").equals("downloading"), "Cancel did not finish");
                command(web, "download_update");
                runOnMainSync(() -> activity.moveTaskToBack(true));
                SystemClock.sleep(2000);
                getTargetContext().startActivity(launch.addFlags(Intent.FLAG_ACTIVITY_REORDER_TO_FRONT));
                await("Activity did not return", 15000, activity::hasWindowFocus);
                await("Public APK download did not finish", 240000, () -> {
                    JSONObject current = command(web, "get_status");
                    require(!current.getString("phase").equals("failed"), "Download failed: " + current.optString("error"));
                    return current.getString("phase").equals("ready");
                });
                state = command(web, "get_status");
                require(state.getLong("downloadedBytes") == state.getJSONObject("release").getLong("size"), "Download size mismatch");
                pass("live public feed, cancel/retry, background download, full APK received");
            } else if (mode.equals("install")) {
                require(state.getString("phase").equals("ready"), "Downloaded APK not recovered across process restart");
                boolean granted = state.getBoolean("canInstall");
                state = command(web, "install_update");
                require(state.getString("phase").equals(granted ? "installing" : "permission"), "Installer did not reach expected system flow");
                pass(granted ? "persisted APK verified and handed to system installer" : "persisted APK verified; missing permission opens system settings");
            } else throw new AssertionError("Unknown test mode");
        } catch (Throwable error) {
            Bundle result = new Bundle();
            result.putString("stream", "\nFAIL " + error.getClass().getSimpleName() + ": " + error.getMessage() + "\n");
            finish(Activity.RESULT_CANCELED, result);
        }
    }
    private void pass(String description) {
        Bundle result = new Bundle(); result.putString("stream", "\nPASS updater: " + description + "\n");
        finish(Activity.RESULT_OK, result);
    }
    private JSONObject command(WebView web, String name) throws Exception {
        js(web, "window.__updateResult=null;window.__TAURI_INTERNALS__.invoke('plugin:memhot-updater|" + name
            + "').then(result=>window.__updateResult={ok:true,result}).catch(e=>window.__updateResult={ok:false,error:String(e)});true");
        await("Command timeout: " + name, 45000, () -> js(web, "window.__updateResult!==null").equals("true"));
        JSONObject result = new JSONObject(js(web, "window.__updateResult"));
        require(result.getBoolean("ok"), "Native " + name + ": " + result.optString("error"));
        return result.getJSONObject("result");
    }
    private WebView find(View view) {
        if (view instanceof WebView) return (WebView) view;
        if (view instanceof ViewGroup) {
            ViewGroup group = (ViewGroup) view;
            for (int i = 0; i < group.getChildCount(); i++) { WebView found = find(group.getChildAt(i)); if (found != null) return found; }
        }
        return null;
    }
    private String js(WebView web, String script) throws Exception {
        CountDownLatch latch = new CountDownLatch(1); String[] value = new String[1];
        runOnMainSync(() -> web.evaluateJavascript(script, result -> { value[0] = result; latch.countDown(); }));
        require(latch.await(5, TimeUnit.SECONDS), "WebView timeout"); return value[0];
    }
    private void await(String message, long timeout, Condition condition) throws Exception {
        long until = SystemClock.elapsedRealtime() + timeout;
        while (!condition.get()) { require(SystemClock.elapsedRealtime() < until, message); SystemClock.sleep(400); }
    }
    private static void require(boolean pass, String message) { if (!pass) throw new AssertionError(message); }
}
