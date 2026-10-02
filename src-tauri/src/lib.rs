mod media;
mod settings;
mod tools;
mod ytdlp;

#[tauri::command]
fn open_file(path: String) -> Result<(), String> {
    tauri_plugin_opener::open_path(path, None::<&str>).map_err(|e| e.to_string())
}

#[tauri::command]
fn show_in_folder(path: String) -> Result<(), String> {
    tauri_plugin_opener::reveal_item_in_dir(path).map_err(|e| e.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(ytdlp::Jobs::default())
        .invoke_handler(tauri::generate_handler![
            settings::get_settings,
            settings::save_settings,
            tools::tools_status,
            tools::tools_install,
            tools::ytdlp_update,
            ytdlp::fetch_info,
            ytdlp::start_download,
            ytdlp::cancel_download,
            media::probe_file,
            media::start_process,
            open_file,
            show_in_folder,
        ])
        .build(tauri::generate_context!())
        .expect("error while building the app")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                ytdlp::cancel_all(app);
            }
        });
}
