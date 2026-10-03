//! Runs yt-dlp: reading media info, and downloading with live progress.
//!
//! yt-dlp is started as a separate program. `--progress-template` and `--print`
//! make it write machine-readable lines (prefixed with `@@`) that are parsed here
//! and forwarded to the UI as `download` events.

use crate::settings::DownloadOptions;
use crate::tools::{self, Tool};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::io::{AsyncRead, AsyncReadExt, BufReader};
use tokio::sync::mpsc;

// ---------------------------------------------------------------------------
// Media info

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum MediaInfo {
    Video(VideoInfo),
    Playlist(PlaylistInfo),
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoInfo {
    url: String,
    title: String,
    channel: Option<String>,
    duration: Option<f64>,
    thumbnail: Option<String>,
    /// Available resolutions, highest first.
    qualities: Vec<Quality>,
    /// Estimated size of the best audio-only download.
    audio_size: Option<u64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Quality {
    /// The short side of the frame in pixels (1080 for both 1920x1080 and a vertical 1080x1920).
    height: u32,
    /// Estimated size of video + audio.
    size: Option<u64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistInfo {
    title: String,
    channel: Option<String>,
    entries: Vec<PlaylistEntry>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistEntry {
    url: String,
    title: String,
    channel: Option<String>,
    duration: Option<f64>,
    thumbnail: Option<String>,
}

fn text(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).filter(|s| !s.is_empty()).map(String::from)
}

fn number(v: &Value, key: &str) -> Option<f64> {
    v.get(key).and_then(Value::as_f64)
}

fn size_of(format: &Value) -> Option<u64> {
    number(format, "filesize")
        .or_else(|| number(format, "filesize_approx"))
        .map(|n| n as u64)
}

fn has_codec(format: &Value, key: &str) -> bool {
    matches!(format.get(key).and_then(Value::as_str), Some(codec) if codec != "none")
}

fn parse_video(v: &Value) -> VideoInfo {
    let formats = v.get("formats").and_then(Value::as_array).map_or(&[][..], Vec::as_slice);

    // Best audio-only stream, preferring m4a like the download does.
    let best_audio = formats
        .iter()
        .filter(|f| has_codec(f, "acodec") && !has_codec(f, "vcodec"))
        .max_by(|a, b| {
            let score = |f: &Value| (text(f, "ext").as_deref() == Some("m4a"), number(f, "abr").unwrap_or(0.0));
            score(a).partial_cmp(&score(b)).unwrap_or(std::cmp::Ordering::Equal)
        });
    let audio_size = best_audio.and_then(size_of);

    // For each resolution, the largest video stream gives a fair size estimate.
    let mut by_height: BTreeMap<u32, Option<u64>> = BTreeMap::new();
    for f in formats.iter().filter(|f| has_codec(f, "vcodec")) {
        let (Some(w), Some(h)) = (number(f, "width"), number(f, "height")) else { continue };
        let short_side = w.min(h) as u32;
        let size = size_of(f).map(|s| if has_codec(f, "acodec") { s } else { s + audio_size.unwrap_or(0) });
        let entry = by_height.entry(short_side).or_insert(None);
        *entry = (*entry).max(size);
    }

    VideoInfo {
        url: text(v, "webpage_url").or_else(|| text(v, "original_url")).unwrap_or_default(),
        title: text(v, "title").unwrap_or_else(|| "Untitled".into()),
        channel: text(v, "channel").or_else(|| text(v, "uploader")),
        duration: number(v, "duration"),
        thumbnail: text(v, "thumbnail"),
        qualities: by_height.into_iter().rev().map(|(height, size)| Quality { height, size }).collect(),
        audio_size,
    }
}

fn parse_playlist(v: &Value) -> PlaylistInfo {
    let entries = v
        .get("entries")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice)
        .iter()
        .filter_map(|e| {
            let title = text(e, "title")?;
            if title == "[Private video]" || title == "[Deleted video]" {
                return None;
            }
            Some(PlaylistEntry {
                url: text(e, "url").or_else(|| text(e, "webpage_url"))?,
                title,
                channel: text(e, "channel").or_else(|| text(e, "uploader")),
                duration: number(e, "duration"),
                thumbnail: e
                    .get("thumbnails")
                    .and_then(Value::as_array)
                    .and_then(|t| t.last())
                    .and_then(|t| text(t, "url"))
                    .or_else(|| text(e, "thumbnail")),
            })
        })
        .collect();

    PlaylistInfo {
        title: text(v, "title").unwrap_or_else(|| "Playlist".into()),
        channel: text(v, "channel").or_else(|| text(v, "uploader")),
        entries,
    }
}

/// The part of a yt-dlp error worth showing: the last ERROR line, without prefixes.
fn error_message(stderr: &str) -> String {
    let line = stderr
        .lines()
        .rev()
        .find(|l| l.starts_with("ERROR:"))
        .or_else(|| stderr.lines().rev().find(|l| !l.trim().is_empty()))
        .unwrap_or("yt-dlp stopped unexpectedly");
    clean_error(line)
}

/// "ERROR: [youtube] abc123: Video unavailable" -> "Video unavailable"
fn clean_error(line: &str) -> String {
    let mut message = line.trim().trim_start_matches("ERROR:").trim();
    if message.starts_with('[') {
        if let Some((_, rest)) = message.split_once("] ") {
            message = rest;
        }
        if let Some((id, rest)) = message.split_once(": ") {
            if !id.contains(' ') {
                message = rest;
            }
        }
    }
    message.to_string()
}

/// Arguments every yt-dlp call gets.
fn base_args(app: &AppHandle) -> Vec<String> {
    let mut args: Vec<String> = ["--ignore-config", "--no-playlist", "--encoding", "utf-8", "--no-colors"]
        .map(String::from)
        .to_vec();
    if let Some((deno, _)) = tools::locate(app, Tool::Deno) {
        args.push("--js-runtimes".into());
        args.push(format!("deno:{}", deno.display()));
    }
    if let Some((ffmpeg, _)) = tools::locate(app, Tool::Ffmpeg) {
        args.push("--ffmpeg-location".into());
        args.push(ffmpeg.display().to_string());
    }
    args
}

fn ytdlp_path(app: &AppHandle) -> Result<PathBuf, String> {
    tools::locate(app, Tool::Ytdlp)
        .map(|(path, _)| path)
        .ok_or_else(|| "yt-dlp isn't installed".into())
}

#[tauri::command]
pub async fn fetch_info(app: AppHandle, url: String, cookies_browser: Option<String>) -> Result<MediaInfo, String> {
    let output = tools::command(&ytdlp_path(&app)?)
        .args(base_args(&app))
        .args(cookies_args(cookies_browser.as_deref().unwrap_or_default()))
        .args(["-J", "--flat-playlist", "--no-warnings", "--", &url])
        .stdin(Stdio::null())
        .output()
        .await
        .map_err(|e| e.to_string())?;

    if !output.status.success() {
        return Err(error_message(&String::from_utf8_lossy(&output.stderr)));
    }
    let v: Value = serde_json::from_slice(&output.stdout).map_err(|_| "yt-dlp returned unreadable info".to_string())?;
    Ok(match v.get("_type").and_then(Value::as_str) {
        Some("playlist") => MediaInfo::Playlist(parse_playlist(&v)),
        _ => MediaInfo::Video(parse_video(&v)),
    })
}

// ---------------------------------------------------------------------------
// Downloads

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadRequest {
    id: String,
    url: String,
    audio_only: bool,
    /// Short side in pixels; `None` means the best available.
    max_height: Option<u32>,
    /// "mp4" | "mkv" | "webm" for video, "mp3" | "m4a" | "opus" for audio.
    format: String,
    folder: String,
    /// Playlist downloads go into a folder named after the playlist.
    subfolder: Option<String>,
    options: DownloadOptions,
}

#[derive(Serialize, Clone)]
#[serde(tag = "type", rename_all = "camelCase")]
pub(crate) enum DownloadEvent {
    #[serde(rename_all = "camelCase")]
    Progress {
        id: String,
        /// Which stream is downloading (0 = video, 1 = audio) when there are several.
        part: usize,
        parts: usize,
        percent: Option<f64>,
        downloaded: Option<u64>,
        total: Option<u64>,
        speed: Option<f64>,
        eta: Option<f64>,
    },
    /// A post-processing step started, e.g. "Merger" or "ExtractAudio".
    Stage { id: String, name: String },
    Log { id: String, line: String },
    #[serde(rename_all = "camelCase")]
    Finished {
        id: String,
        ok: bool,
        canceled: bool,
        filepath: Option<String>,
        error: Option<String>,
    },
}

#[derive(Default)]
pub struct Jobs {
    /// Job id -> (process id, staging folder). Shared with `media.rs`, which runs FFmpeg jobs.
    pub(crate) running: Mutex<HashMap<String, (u32, PathBuf)>>,
    pub(crate) canceled: Mutex<HashSet<String>>,
}

fn format_args(req: &DownloadRequest) -> Vec<String> {
    let format = req.format.as_str();
    if req.audio_only {
        let selector = match format {
            "m4a" => "ba[ext=m4a]/ba/b",
            "opus" => "ba[acodec=opus]/ba/b",
            _ => "ba/b",
        };
        return ["-f", selector, "-x", "--audio-format", format, "--audio-quality", "0"]
            .map(String::from)
            .to_vec();
    }

    // -S picks the best stream up to the chosen resolution, preferring the chosen container.
    // MP4 also prefers H.264, which plays on practically every device (AV1/VP9 only when that's all there is).
    let res = req.max_height.map_or("res".to_string(), |h| format!("res:{h}"));
    let sort = match format {
        "mp4" => format!("{res},vcodec:h264,ext:mp4:m4a"),
        "webm" => format!("{res},ext:webm:webm"),
        _ => res,
    };
    let merge = if format == "webm" { "webm/mkv" } else { format };
    let mut args = vec!["-f".into(), "bv*+ba/b".into(), "-S".into(), sort, "--merge-output-format".into(), merge.into()];
    if format != "webm" {
        args.extend(["--remux-video".into(), format.into()]);
    }
    args
}

fn safe_folder_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if c.is_control() || r#"<>:"/\|?*"#.contains(c) { ' ' } else { c })
        .collect();
    let cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let cleaned = cleaned.trim_matches(|c: char| c == '.' || c == ' ');
    if cleaned.is_empty() { "Playlist".into() } else { cleaned.chars().take(120).collect() }
}

const COOKIE_BROWSERS: [&str; 8] = ["chrome", "edge", "firefox", "brave", "chromium", "opera", "vivaldi", "safari"];

fn cookies_args(browser: &str) -> Vec<String> {
    if COOKIE_BROWSERS.contains(&browser) {
        vec!["--cookies-from-browser".into(), browser.into()]
    } else {
        Vec::new()
    }
}

/// "2.5" + "M" -> "2.5M". Empty, zero or nonsense means no limit.
fn rate_limit(o: &DownloadOptions) -> Option<String> {
    let value: f64 = o.rate_limit_value.trim().replace(',', ".").parse().ok()?;
    if !value.is_finite() || value <= 0.0 {
        return None;
    }
    Some(format!("{value}{}", if o.rate_limit_unit == "K" { 'K' } else { 'M' }))
}

/// yt-dlp arguments for the user's options. Custom arguments come last so they can override the rest.
fn option_args(req: &DownloadRequest) -> Result<Vec<String>, String> {
    let o = &req.options;
    let mut a: Vec<String> = Vec::new();
    let mut add = |args: &[&str]| a.extend(args.iter().map(|s| s.to_string()));

    if let Some(limit) = rate_limit(o) {
        add(&["-r", &limit]);
    }
    if o.set_file_time_now {
        add(&["--no-mtime"]);
    }
    if o.embed_metadata {
        add(&["--embed-metadata"]);
    }
    match o.chapters.as_str() {
        "split" => {
            add(&["--split-chapters", "-o", "chapter:%(title)s - %(section_number)03d %(section_title)s.%(ext)s"]);
            if o.force_keyframes {
                add(&["--force-keyframes-at-cuts"]);
            }
        }
        "ignore" => add(&["--no-embed-chapters"]),
        // Embedding chapters is part of --embed-metadata.
        _ if !o.embed_metadata => add(&["--embed-chapters"]),
        _ => {}
    }
    if !req.audio_only && o.subtitles != "off" {
        let langs = o.subtitle_langs.trim();
        add(&["--write-subs", "--write-auto-subs", "--sub-langs", if langs.is_empty() { "en" } else { langs }]);
        if o.subtitles == "embed" {
            add(&["--embed-subs"]);
        } else {
            add(&["--convert-subs", "srt"]);
        }
    }
    // WebM can't hold cover art; yt-dlp would fail the whole download.
    if o.embed_thumbnail && (req.audio_only || matches!(req.format.as_str(), "mp4" | "mkv")) {
        add(&["--embed-thumbnail"]);
    }
    if o.sponsorblock {
        add(&["--sponsorblock-remove", "sponsor"]);
    }
    a.extend(cookies_args(&o.cookies_browser));
    if o.custom_args_enabled && !o.custom_args.trim().is_empty() {
        a.extend(shell_words::split(&o.custom_args).map_err(|e| format!("Custom arguments: {e}"))?);
    }
    Ok(a)
}

fn download_args(app: &AppHandle, req: &DownloadRequest, staging: &Path) -> Result<Vec<String>, String> {
    let mut args = base_args(app);
    args.extend(
        [
            "--newline",
            "--no-quiet",
            "--progress",
            "--progress-delta",
            "0.5",
            "--progress-template",
            r#"download:@@P {"p":%(progress)j,"f":%(info.format_id)j}"#,
            "--progress-template",
            r#"postprocess:@@S {"s":%(progress.status)j,"p":%(progress.postprocessor)j}"#,
            "--print",
            "after_move:@@F %(filepath)j",
        ]
        .map(String::from),
    );
    args.extend(format_args(req));
    args.extend(["-P".into(), staging.display().to_string(), "-o".into(), "%(title)s.%(ext)s".into()]);
    args.extend(option_args(req)?);
    args.extend(["--".into(), req.url.clone()]);
    Ok(args)
}

// Each download runs in its own hidden folder next to the destination and is moved into place
// when it finishes. yt-dlp would otherwise skip a download whose file name already exists (the same
// video in another quality, or two videos with the same title), and a canceled download leaves no
// partial files behind.

const STAGING: &str = ".ytdlp-ui";

pub(crate) fn staging_dir(folder: &Path, id: &str) -> Result<PathBuf, String> {
    let root = folder.join(STAGING);
    let short_id: String = id.chars().filter(char::is_ascii_alphanumeric).take(8).collect();
    let dir = root.join(short_id);
    std::fs::create_dir_all(&dir).map_err(|e| format!("Couldn't write to the download folder: {e}"))?;
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new("attrib").arg("+h").arg(&root).creation_flags(0x0800_0000).status();
    }
    Ok(dir)
}

/// Partial and scratch files yt-dlp leaves in the folder while it works.
fn is_temporary(name: &str) -> bool {
    [".part", ".ytdl", ".temp", ".tmp"].iter().any(|ext| name.ends_with(ext)) || name.contains(".part-")
}

/// Where `name` can go in `dir` without replacing anything: "Title.mp4", then "Title (1).mp4", ...
fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let path = Path::new(name);
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let ext = path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    let mut target = dir.join(name);
    let mut n = 1;
    while target.exists() {
        target = dir.join(format!("{stem} ({n}){ext}"));
        n += 1;
    }
    target
}

/// Moves everything a download produced (the video, subtitle files, split chapters...) from `staging` into
/// `dir` and returns the new path of `main`. Taken names get " (1)", " (2)"... like a browser does; a video and
/// its sidecar files (`Title.mp4`, `Title.en.srt`) always get the same number so they stay paired.
pub(crate) fn move_results(main: &Path, staging: &Path, dir: &Path) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let main_name = main.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let stem = main.file_stem().unwrap_or_default().to_string_lossy().into_owned();
    let main_rest = main_name[stem.len()..].to_string();

    let mut paired: Vec<(String, PathBuf)> = Vec::new(); // (text after the stem, file)
    let mut others: Vec<(String, PathBuf)> = Vec::new(); // (file name, file)
    for entry in std::fs::read_dir(staging)?.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if !path.is_file() || is_temporary(&name) {
            continue;
        }
        match name.strip_prefix(stem.as_str()).filter(|rest| rest.starts_with('.')) {
            Some(rest) => paired.push((rest.to_string(), path)),
            None => others.push((name, path)),
        }
    }
    let main_exists = paired.iter().any(|(_, path)| path == main) || main.is_file();
    if main_exists && !paired.iter().any(|(_, path)| path == main) {
        paired.push((main_rest.clone(), main.to_path_buf())); // it lives outside the staging folder
    }

    let named = |n: u32| if n == 0 { stem.clone() } else { format!("{stem} ({n})") };
    let mut n = 0;
    while paired.iter().any(|(rest, _)| dir.join(format!("{}{rest}", named(n))).exists()) {
        n += 1;
    }
    for (rest, path) in &paired {
        std::fs::rename(path, dir.join(format!("{}{rest}", named(n))))?;
    }
    let mut moved_others = Vec::new();
    for (name, path) in &others {
        let target = unique_path(dir, name);
        std::fs::rename(path, &target)?;
        moved_others.push(target);
    }
    // Normally the file yt-dlp reported; if it isn't there, whatever else came out (split chapters).
    Ok(if main_exists {
        dir.join(format!("{}{main_rest}", named(n)))
    } else {
        moved_others.into_iter().next().unwrap_or_else(|| dir.to_path_buf())
    })
}

/// Removes a job's staging folder. Windows can hold files open briefly after a process is killed.
pub(crate) fn remove_staging(dir: &Path) {
    for _ in 0..20 {
        if std::fs::remove_dir_all(dir).is_ok() || !dir.exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    if let Some(root) = dir.parent() {
        let _ = std::fs::remove_dir(root); // only succeeds once no other download is using it
    }
}

#[tauri::command]
pub async fn start_download(app: AppHandle, jobs: State<'_, Jobs>, req: DownloadRequest) -> Result<(), String> {
    let folder = PathBuf::from(&req.folder);
    if !folder.is_dir() {
        return Err("The download folder doesn't exist".into());
    }
    let destination = match &req.subfolder {
        Some(sub) => folder.join(safe_folder_name(sub)),
        None => folder.clone(),
    };
    let ytdlp = ytdlp_path(&app)?;
    let staging = staging_dir(&folder, &req.id)?;
    let args = match download_args(&app, &req, &staging) {
        Ok(args) => args,
        Err(e) => {
            remove_staging(&staging);
            return Err(e);
        }
    };

    let spawned = tools::command(&ytdlp)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(e) => {
            remove_staging(&staging);
            return Err(format!("Couldn't start yt-dlp: {e}"));
        }
    };

    let id = req.id;
    if let Some(pid) = child.id() {
        jobs.running.lock().unwrap().insert(id.clone(), (pid, staging.clone()));
    }

    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    forward_lines(child.stdout.take(), tx.clone());
    forward_lines(child.stderr.take(), tx);

    tauri::async_runtime::spawn(async move {
        let mut parser = OutputParser::default();
        while let Some(line) = rx.recv().await {
            if let Some(event) = parser.parse(&id, &line) {
                let _ = app.emit("download", event);
            }
        }
        let success = child.wait().await.map(|s| s.success()).unwrap_or(false);

        let jobs = app.state::<Jobs>();
        jobs.running.lock().unwrap().remove(&id);
        let canceled = jobs.canceled.lock().unwrap().remove(&id);

        let result = match (canceled, success, parser.filepath) {
            (true, _, _) => Err(None),
            (false, true, Some(file)) => move_results(Path::new(&file), &staging, &destination)
                .map(|path| path.display().to_string())
                .map_err(|e| Some(format!("Couldn't save the file: {e}"))),
            (false, true, None) => Err(Some("yt-dlp didn't report the downloaded file".into())),
            (false, false, _) => Err(Some(parser.error.unwrap_or_else(|| "yt-dlp stopped unexpectedly".into()))),
        };
        let _ = tokio::task::spawn_blocking(move || remove_staging(&staging)).await;

        let _ = app.emit(
            "download",
            DownloadEvent::Finished {
                ok: result.is_ok(),
                canceled,
                filepath: result.as_ref().ok().cloned(),
                error: result.err().flatten(),
                id,
            },
        );
    });
    Ok(())
}

pub(crate) fn forward_lines(stream: Option<impl AsyncRead + Unpin + Send + 'static>, tx: mpsc::UnboundedSender<String>) {
    let Some(stream) = stream else { return };
    tauri::async_runtime::spawn(async move {
        // Progress bars (such as Python's tqdm) redraw a line with a carriage return, so both end a line.
        let mut reader = BufReader::new(stream);
        let mut line: Vec<u8> = Vec::new();
        let mut chunk = [0u8; 4096];
        let send = |line: &mut Vec<u8>| -> bool {
            let text = String::from_utf8_lossy(line).trim_end().to_string();
            line.clear();
            text.is_empty() || tx.send(text).is_ok()
        };
        loop {
            match reader.read(&mut chunk).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    for &byte in &chunk[..n] {
                        if byte == b'\n' || byte == b'\r' {
                            if !send(&mut line) {
                                return;
                            }
                        } else {
                            line.push(byte);
                        }
                    }
                }
            }
        }
        send(&mut line);
    });
}

#[derive(Default)]
struct OutputParser {
    /// Format ids being downloaded, e.g. ["137", "140"] for video + audio.
    formats: Vec<String>,
    filepath: Option<String>,
    error: Option<String>,
}

impl OutputParser {
    fn parse(&mut self, id: &str, line: &str) -> Option<DownloadEvent> {
        let id = id.to_string();
        if let Some(json) = line.strip_prefix("@@P ") {
            let v: Value = serde_json::from_str(json).ok()?;
            let p = v.get("p")?;
            let format_id = v.get("f").and_then(Value::as_str).unwrap_or_default();
            let total = number(p, "total_bytes").or_else(|| number(p, "total_bytes_estimate"));
            let downloaded = number(p, "downloaded_bytes");
            let percent = number(p, "_percent").or_else(|| Some(downloaded? / total? * 100.0));
            return Some(DownloadEvent::Progress {
                id,
                part: self.formats.iter().position(|f| f == format_id).unwrap_or(0),
                parts: self.formats.len().max(1),
                percent: percent.map(|p| p.clamp(0.0, 100.0)),
                downloaded: downloaded.map(|n| n as u64),
                total: total.map(|n| n as u64),
                speed: number(p, "speed"),
                eta: number(p, "eta"),
            });
        }
        if let Some(json) = line.strip_prefix("@@S ") {
            let v: Value = serde_json::from_str(json).ok()?;
            if v.get("s").and_then(Value::as_str) != Some("started") {
                return None;
            }
            return Some(DownloadEvent::Stage { id, name: text(&v, "p")? });
        }
        if let Some(json) = line.strip_prefix("@@F ") {
            self.filepath = serde_json::from_str(json).ok();
            return None;
        }
        if line.starts_with("ERROR:") {
            self.error = Some(clean_error(line));
        } else if let Some(list) = line.strip_prefix("[info] ").and_then(|l| l.split_once(": Downloading ")) {
            // "[info] abc123: Downloading 1 format(s): 137+140"
            if let Some((_, ids)) = list.1.split_once("format(s): ") {
                self.formats = ids.split('+').map(|s| s.trim().to_string()).collect();
            }
        }
        Some(DownloadEvent::Log { id, line: line.to_string() })
    }
}

#[tauri::command]
pub fn cancel_download(jobs: State<'_, Jobs>, id: String) {
    let pid = jobs.running.lock().unwrap().get(&id).map(|(pid, _)| *pid);
    if let Some(pid) = pid {
        jobs.canceled.lock().unwrap().insert(id);
        kill_tree(pid, false);
    }
}

/// Stops everything still running and removes its partial files, e.g. when the app closes.
pub fn cancel_all(app: &AppHandle) {
    let jobs = app.state::<Jobs>();
    let running: Vec<(u32, PathBuf)> = jobs.running.lock().unwrap().values().cloned().collect();
    for (pid, staging) in running {
        kill_tree(pid, true);
        remove_staging(&staging);
    }
}

/// yt-dlp starts ffmpeg itself, so the whole process tree has to go.
pub(crate) fn kill_tree(pid: u32, wait: bool) {
    let kill = move || {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            let _ = std::process::Command::new("taskkill")
                .args(["/PID", &pid.to_string(), "/T", "/F"])
                .creation_flags(0x0800_0000)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        #[cfg(unix)]
        {
            let pid = pid.to_string();
            let _ = std::process::Command::new("pkill").args(["-TERM", "-P", &pid]).status();
            let _ = std::process::Command::new("kill").args(["-TERM", &pid]).status();
        }
    };
    if wait {
        kill();
    } else {
        std::thread::spawn(kill);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_error_prefixes() {
        assert_eq!(clean_error("ERROR: [youtube] abc123: Video unavailable"), "Video unavailable");
        assert_eq!(clean_error("ERROR: Unsupported URL: https://x.y"), "Unsupported URL: https://x.y");
    }

    #[test]
    fn parses_progress_and_formats() {
        let mut parser = OutputParser::default();
        parser.parse("1", "[info] abc: Downloading 1 format(s): 137+140");
        assert_eq!(parser.formats, ["137", "140"]);

        let line = r#"@@P {"p":{"status":"downloading","downloaded_bytes":50,"total_bytes":200,"speed":10.0,"eta":15,"_percent":25.0},"f":"140"}"#;
        match parser.parse("1", line) {
            Some(DownloadEvent::Progress { part, parts, percent, total, .. }) => {
                assert_eq!((part, parts, percent, total), (1, 2, Some(25.0), Some(200)));
            }
            _ => panic!("expected progress"),
        }

        assert!(parser.parse("1", r#"@@F "C:\\Videos\\a.mp4""#).is_none());
        assert_eq!(parser.filepath.as_deref(), Some(r"C:\Videos\a.mp4"));
    }

    fn scratch_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ytdlp-ui-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn moving_keeps_files_paired_and_never_overwrites() {
        let root = scratch_dir("move");
        let (staging, dest) = (root.join("staging"), root.join("dest"));
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(dest.join("a.mp4"), "old").unwrap();
        for name in ["a.mp4", "a.en.srt", "a - 001 Intro.mp4", "b.mp4.part"] {
            std::fs::write(staging.join(name), name).unwrap();
        }

        let moved = move_results(&staging.join("a.mp4"), &staging, &dest).unwrap();
        assert_eq!(moved, dest.join("a (1).mp4"));
        assert_eq!(std::fs::read_to_string(dest.join("a.mp4")).unwrap(), "old");
        assert!(dest.join("a (1).en.srt").exists(), "subtitles share the video's number");
        assert!(dest.join("a - 001 Intro.mp4").exists());
        assert!(staging.join("b.mp4.part").exists(), "partial files stay behind");
        std::fs::remove_dir_all(&root).unwrap();
    }

    fn request(audio_only: bool, format: &str, options: DownloadOptions) -> DownloadRequest {
        DownloadRequest {
            id: "1".into(),
            url: "https://example.com/v".into(),
            audio_only,
            max_height: None,
            format: format.into(),
            folder: String::new(),
            subfolder: None,
            options,
        }
    }

    #[test]
    fn options_become_arguments() {
        let o = DownloadOptions {
            rate_limit_value: "2,5".into(),
            rate_limit_unit: "M".into(),
            subtitles: "embed".into(),
            subtitle_langs: "en,de".into(),
            chapters: "split".into(),
            force_keyframes: true,
            sponsorblock: true,
            cookies_browser: "firefox".into(),
            custom_args_enabled: true,
            custom_args: r#"--proxy "http://a b:1""#.into(),
            ..DownloadOptions::default()
        };
        let args = option_args(&request(false, "mp4", o)).unwrap();
        let has = |pair: &[&str]| args.windows(pair.len()).any(|w| w == pair);
        assert!(has(&["-r", "2.5M"]));
        assert!(has(&["--sub-langs", "en,de"]) && args.contains(&"--embed-subs".to_string()));
        assert!(args.contains(&"--split-chapters".to_string()) && args.contains(&"--force-keyframes-at-cuts".to_string()));
        assert!(has(&["--sponsorblock-remove", "sponsor"]));
        assert!(has(&["--cookies-from-browser", "firefox"]));
        assert!(args.contains(&"--embed-thumbnail".to_string()));
        assert_eq!(&args[args.len() - 2..], ["--proxy", "http://a b:1"], "custom arguments come last, kept whole inside quotes");
    }

    #[test]
    fn options_respect_the_format() {
        let o = DownloadOptions { subtitles: "embed".into(), ..DownloadOptions::default() };
        let webm = option_args(&request(false, "webm", o.clone())).unwrap();
        assert!(!webm.contains(&"--embed-thumbnail".to_string()), "webm can't hold cover art");
        let mp3 = option_args(&request(true, "mp3", o)).unwrap();
        assert!(mp3.contains(&"--embed-thumbnail".to_string()));
        assert!(!mp3.contains(&"--write-subs".to_string()), "no subtitles for audio");

        let bad = DownloadOptions { custom_args_enabled: true, custom_args: "--x \"unclosed".into(), ..DownloadOptions::default() };
        assert!(option_args(&request(false, "mp4", bad)).is_err());
        let junk = DownloadOptions { rate_limit_value: "fast".into(), cookies_browser: "notabrowser".into(), ..DownloadOptions::default() };
        let args = option_args(&request(false, "mp4", junk)).unwrap();
        assert!(!args.contains(&"-r".to_string()) && !args.contains(&"--cookies-from-browser".to_string()));
    }

    #[test]
    fn folder_names_are_safe() {
        assert_eq!(safe_folder_name(r#"Best: of "2026" / mix?"#), "Best of 2026 mix");
        assert_eq!(safe_folder_name("..."), "Playlist");
    }
}
