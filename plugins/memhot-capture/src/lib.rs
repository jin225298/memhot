//! Native microphone capture. Android implementation is independent of WebView lifetime.

use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use tauri::{
    plugin::{Builder, TauriPlugin},
    Manager, Runtime, State,
};

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureConfig {
    pub base_url: String,
    pub token: String,
    #[serde(default)]
    pub user_id: String,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            base_url: "https://memhot.angelkang.com".into(),
            token: String::new(),
            user_id: String::new(),
        }
    }
}

struct Capture<R: Runtime> {
    #[cfg(target_os = "android")]
    handle: tauri::plugin::PluginHandle<R>,
    #[cfg(not(target_os = "android"))]
    config: Mutex<CaptureConfig>,
    #[cfg(not(target_os = "android"))]
    runtime: std::marker::PhantomData<fn() -> R>,
}

impl<R: Runtime> Capture<R> {
    fn call(&self, command: &str, args: serde_json::Value) -> Result<serde_json::Value, String> {
        #[cfg(target_os = "android")]
        {
            self.handle
                .run_mobile_plugin(command, args)
                .map_err(|error| match error {
                    tauri::plugin::mobile::PluginInvokeError::InvokeRejected(response) => response
                        .message
                        .filter(|message| !message.trim().is_empty())
                        .unwrap_or_else(|| "设备操作未完成，请重试".into()),
                    // Never expose raw JNI diagnostics or serialized arguments.
                    _ => "无法连接设备录音功能，请关闭应用后重新打开".into(),
                })
        }
        #[cfg(not(target_os = "android"))]
        {
            match command {
                "configure" => {
                    let config: CaptureConfig = serde_json::from_value(args)
                        .map_err(|_| "Invalid capture configuration")?;
                    *self
                        .config
                        .lock()
                        .map_err(|_| "Configuration unavailable")? = config;
                    Ok(serde_json::json!({}))
                }
                "getConfig" => serde_json::to_value(
                    self.config
                        .lock()
                        .map_err(|_| "Configuration unavailable")?
                        .clone(),
                )
                .map_err(|_| "Configuration unavailable".into()),
                "clearPending" => Ok(serde_json::json!({"clearedCount":0})),
                "getLaunchTarget" => Ok(serde_json::json!({"tab":null})),
                "status" | "stop" => Ok(
                    serde_json::json!({"supported":false,"state":"idle","elapsedMs":0,"queuedCount":0,"uploadedCount":0}),
                ),
                _ => Err("Background microphone capture is available on Android only".into()),
            }
        }
    }
}

async fn call<R: Runtime>(
    state: State<'_, Capture<R>>,
    name: &str,
    args: serde_json::Value,
) -> Result<serde_json::Value, String> {
    state.call(name, args)
}

#[tauri::command]
async fn configure<R: Runtime>(
    _app: tauri::AppHandle<R>,
    state: State<'_, Capture<R>>,
    base_url: String,
    token: String,
    user_id: Option<String>,
) -> Result<serde_json::Value, String> {
    call(
        state,
        "configure",
        serde_json::json!({"baseUrl":base_url,"token":token,"userId":user_id.unwrap_or_default()}),
    )
    .await
}

#[tauri::command]
async fn get_config<R: Runtime>(
    _app: tauri::AppHandle<R>,
    state: State<'_, Capture<R>>,
) -> Result<serde_json::Value, String> {
    call(state, "getConfig", serde_json::json!({})).await
}

#[tauri::command]
async fn start<R: Runtime>(
    _app: tauri::AppHandle<R>,
    state: State<'_, Capture<R>>,
) -> Result<serde_json::Value, String> {
    call(state, "start", serde_json::json!({})).await
}

#[tauri::command]
async fn stop<R: Runtime>(
    _app: tauri::AppHandle<R>,
    state: State<'_, Capture<R>>,
) -> Result<serde_json::Value, String> {
    call(state, "stop", serde_json::json!({})).await
}

#[tauri::command]
async fn status<R: Runtime>(
    _app: tauri::AppHandle<R>,
    state: State<'_, Capture<R>>,
) -> Result<serde_json::Value, String> {
    call(state, "status", serde_json::json!({})).await
}

#[tauri::command]
async fn open_battery_settings<R: Runtime>(
    _app: tauri::AppHandle<R>,
    state: State<'_, Capture<R>>,
) -> Result<serde_json::Value, String> {
    call(state, "openBatterySettings", serde_json::json!({})).await
}

#[tauri::command]
async fn clear_pending<R: Runtime>(
    _app: tauri::AppHandle<R>,
    state: State<'_, Capture<R>>,
) -> Result<serde_json::Value, String> {
    call(state, "clearPending", serde_json::json!({})).await
}

#[tauri::command]
async fn request_notifications<R: Runtime>(
    _app: tauri::AppHandle<R>,
    state: State<'_, Capture<R>>,
) -> Result<serde_json::Value, String> {
    call(state, "requestNotifications", serde_json::json!({})).await
}

#[tauri::command]
async fn set_asr_mode<R: Runtime>(
    _app: tauri::AppHandle<R>,
    state: State<'_, Capture<R>>,
    mode: String,
) -> Result<serde_json::Value, String> {
    call(state, "setAsrMode", serde_json::json!({"mode":mode})).await
}
#[tauri::command]
async fn get_launch_target<R: Runtime>(
    _app: tauri::AppHandle<R>,
    state: State<'_, Capture<R>>,
) -> Result<serde_json::Value, String> {
    call(state, "getLaunchTarget", serde_json::json!({})).await
}
#[tauri::command]
async fn download_local_model<R: Runtime>(
    _app: tauri::AppHandle<R>,
    state: State<'_, Capture<R>>,
) -> Result<serde_json::Value, String> {
    call(state, "downloadLocalModel", serde_json::json!({})).await
}
#[tauri::command]
async fn cancel_local_model_download<R: Runtime>(
    _app: tauri::AppHandle<R>,
    state: State<'_, Capture<R>>,
) -> Result<serde_json::Value, String> {
    call(state, "cancelLocalModelDownload", serde_json::json!({})).await
}
#[tauri::command]
async fn remove_local_model<R: Runtime>(
    _app: tauri::AppHandle<R>,
    state: State<'_, Capture<R>>,
) -> Result<serde_json::Value, String> {
    call(state, "removeLocalModel", serde_json::json!({})).await
}
#[tauri::command]
async fn retry_pending<R: Runtime>(
    _app: tauri::AppHandle<R>,
    state: State<'_, Capture<R>>,
    accept_duplicate_charge: Option<bool>,
) -> Result<serde_json::Value, String> {
    call(
        state,
        "retryPending",
        serde_json::json!({"acceptDuplicateCharge":accept_duplicate_charge.unwrap_or(false)}),
    )
    .await
}

pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("memhot-capture")
        .invoke_handler(tauri::generate_handler![
            configure,
            get_config,
            start,
            stop,
            status,
            open_battery_settings,
            clear_pending,
            request_notifications,
            set_asr_mode,
            download_local_model,
            cancel_local_model_download,
            remove_local_model,
            retry_pending,
            get_launch_target
        ])
        .setup(|app, api| {
            #[cfg(target_os = "android")]
            let capture = Capture {
                handle: api.register_android_plugin(
                    "com.angelkang.memhot.capture",
                    "MemhotCapturePlugin",
                )?,
            };
            #[cfg(not(target_os = "android"))]
            let capture = {
                let _ = api;
                Capture {
                    config: Mutex::new(CaptureConfig::default()),
                    runtime: std::marker::PhantomData::<fn() -> R>,
                }
            };
            app.manage(capture);
            Ok(())
        })
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn desktop_status_does_not_claim_microphone_support() {
        let capture = Capture::<tauri::test::MockRuntime> {
            config: Mutex::new(CaptureConfig::default()),
            runtime: std::marker::PhantomData,
        };
        assert_eq!(
            capture.call("status", serde_json::json!({})).unwrap()["supported"],
            false
        );
        assert!(capture.call("start", serde_json::json!({})).is_err());
    }
}
