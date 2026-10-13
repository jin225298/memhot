const COMMANDS: &[&str] = &[
    "configure",
    "get_config",
    "start",
    "stop",
    "status",
    "open_battery_settings",
    "clear_pending",
    "request_notifications",
    "set_asr_mode",
    "download_local_model",
    "cancel_local_model_download",
    "remove_local_model",
    "retry_pending",
    "get_launch_target",
];

fn main() {
    tauri_plugin::Builder::new(COMMANDS)
        .android_path("android")
        .build();
}
