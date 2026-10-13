const COMMANDS: &[&str] = &[
    "get_status",
    "check_update",
    "download_update",
    "cancel_download",
    "install_update",
];

fn main() {
    tauri_plugin::Builder::new(COMMANDS)
        .android_path("android")
        .build();
}
