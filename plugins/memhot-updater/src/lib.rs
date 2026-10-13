//! Android package updates from the fixed Memhot release channel.
//! The native boundary accepts no download URLs, filesystem paths, or release metadata.

use tauri::{
    plugin::{Builder, TauriPlugin},
    Manager, Runtime, State,
};

struct Updater<R: Runtime> {
    #[cfg(target_os = "android")]
    handle: tauri::plugin::PluginHandle<R>,
    #[cfg(not(target_os = "android"))]
    runtime: std::marker::PhantomData<fn() -> R>,
}

impl<R: Runtime> Updater<R> {
    fn call(&self, command: &str, app: &tauri::AppHandle<R>) -> Result<serde_json::Value, String> {
        #[cfg(target_os = "android")]
        {
            let _ = app;
            self.handle
                .run_mobile_plugin(command, serde_json::json!({}))
                .map_err(|error| match error {
                    tauri::plugin::mobile::PluginInvokeError::InvokeRejected(response) => response
                        .message
                        .filter(|message| !message.trim().is_empty())
                        .unwrap_or_else(|| "更新操作未完成，请重试".into()),
                    _ => "无法连接设备更新功能，请关闭应用后重新打开".into(),
                })
        }
        #[cfg(not(target_os = "android"))]
        {
            let _ = command;
            Ok(desktop_status(&app.package_info().version.to_string()))
        }
    }
}

#[cfg(not(target_os = "android"))]
fn desktop_status(version: &str) -> serde_json::Value {
    serde_json::json!({
        "supported": false, "currentVersion": version, "currentVersionCode": 0,
        "phase": "idle", "release": null, "downloadedBytes": 0, "totalBytes": 0,
        "canInstall": false, "error": null
    })
}

#[tauri::command]
async fn get_status<R: Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, Updater<R>>,
) -> Result<serde_json::Value, String> {
    state.call("getStatus", &app)
}
#[tauri::command]
async fn check_update<R: Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, Updater<R>>,
) -> Result<serde_json::Value, String> {
    state.call("checkUpdate", &app)
}
#[tauri::command]
async fn download_update<R: Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, Updater<R>>,
) -> Result<serde_json::Value, String> {
    state.call("downloadUpdate", &app)
}
#[tauri::command]
async fn cancel_download<R: Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, Updater<R>>,
) -> Result<serde_json::Value, String> {
    state.call("cancelDownload", &app)
}
#[tauri::command]
async fn install_update<R: Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, Updater<R>>,
) -> Result<serde_json::Value, String> {
    state.call("installUpdate", &app)
}

pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("memhot-updater")
        .invoke_handler(tauri::generate_handler![
            get_status,
            check_update,
            download_update,
            cancel_download,
            install_update
        ])
        .setup(|app, api| {
            #[cfg(target_os = "android")]
            let updater = Updater {
                handle: api.register_android_plugin(
                    "com.angelkang.memhot.updater",
                    "MemhotUpdaterPlugin",
                )?,
            };
            #[cfg(not(target_os = "android"))]
            let updater = {
                let _ = api;
                Updater {
                    runtime: std::marker::PhantomData::<fn() -> R>,
                }
            };
            app.manage(updater);
            Ok(())
        })
        .build()
}

#[cfg(all(test, not(target_os = "android")))]
mod tests {
    use super::*;

    #[test]
    fn desktop_reports_native_updates_unavailable() {
        let state = desktop_status("0.3.3");
        assert_eq!(state["supported"], false);
        assert_eq!(state["phase"], "idle");
        assert_eq!(state["canInstall"], false);
        assert!(state["release"].is_null());
    }
}
