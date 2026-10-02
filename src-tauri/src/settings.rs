use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

/// How downloads are made. Read when a download starts, so changes apply to everything still waiting.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct DownloadOptions {
    /// Empty means no limit.
    pub rate_limit_value: String,
    /// "K" | "M"
    pub rate_limit_unit: String,
    /// Give the file the time it was downloaded instead of the upload date.
    pub set_file_time_now: bool,
    /// "off" | "embed" | "file"
    pub subtitles: String,
    pub subtitle_langs: String,
    /// "embed" | "split" | "ignore"
    pub chapters: String,
    pub force_keyframes: bool,
    pub embed_thumbnail: bool,
    pub embed_metadata: bool,
    pub sponsorblock: bool,
    /// "" or a browser name such as "firefox".
    pub cookies_browser: String,
    pub custom_args_enabled: bool,
    pub custom_args: String,
    /// Even out the volume of converted local files.
    pub normalize_audio: bool,
}

impl Default for DownloadOptions {
    fn default() -> Self {
        Self {
            rate_limit_value: String::new(),
            rate_limit_unit: "M".into(),
            set_file_time_now: true,
            subtitles: "off".into(),
            subtitle_langs: "en".into(),
            chapters: "embed".into(),
            force_keyframes: false,
            embed_thumbnail: true,
            embed_metadata: true,
            sponsorblock: false,
            cookies_browser: String::new(),
            custom_args_enabled: false,
            custom_args: String::new(),
            normalize_audio: false,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub download_dir: String,
    pub max_concurrent: u32,
    /// "system" | "light" | "dark"
    pub theme: String,
    pub auto_update: bool,
    /// Unix seconds of the last automatic yt-dlp update check.
    pub last_update_check: u64,
    /// Start downloading as soon as a link has been added.
    pub auto_start: bool,
    /// Quality and format given to newly added links, remembered from the last change.
    pub quality: String,
    pub video_format: String,
    pub audio_format: String,
    /// Whether the panel under the queue is open, and which tab ("options" | "output") it shows.
    pub panel_open: bool,
    pub panel_tab: String,
    pub options: DownloadOptions,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            download_dir: String::new(),
            max_concurrent: 2,
            theme: "system".into(),
            auto_update: true,
            last_update_check: 0,
            auto_start: false,
            quality: "best".into(),
            video_format: "mp4".into(),
            audio_format: "mp3".into(),
            panel_open: true,
            panel_tab: "options".into(),
            options: DownloadOptions::default(),
        }
    }
}

fn settings_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|dir| dir.join("settings.json"))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_settings(app: AppHandle) -> Settings {
    let mut settings: Settings = settings_path(&app)
        .ok()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();

    if settings.download_dir.is_empty() {
        let dir = app.path().download_dir().or_else(|_| app.path().home_dir());
        settings.download_dir = dir.map(|d| d.to_string_lossy().into_owned()).unwrap_or_default();
    }
    settings.max_concurrent = settings.max_concurrent.clamp(1, 5);
    settings
}

#[tauri::command]
pub fn save_settings(app: AppHandle, settings: Settings) -> Result<(), String> {
    let path = settings_path(&app)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?;
    std::fs::write(path, text).map_err(|e| e.to_string())
}
