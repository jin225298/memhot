//! Memhot application entry point.

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(
            tauri_plugin_opener::Builder::new()
                .open_js_links_on_click(false)
                .build(),
        )
        .plugin(tauri_plugin_memhot_capture::init())
        .plugin(tauri_plugin_memhot_updater::init())
        .run(tauri::generate_context!())
        .expect("error while running Memhot");
}
