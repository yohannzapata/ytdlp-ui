//! Runs yt-dlp: reading media info, and downloading with live progress.
//!
//! yt-dlp is started as a separate program. `--progress-template` and `--print`
//! make it write machine-readable lines (prefixed with `@@`) that are parsed here
//! and forwarded to the UI as `download` events.

use crate::tools::{self, Tool};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
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
pub async fn fetch_info(app: AppHandle, url: String) -> Result<MediaInfo, String> {
    let output = tools::command(&ytdlp_path(&app)?)
        .args(base_args(&app))
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
}

#[derive(Serialize, Clone)]
#[serde(tag = "type", rename_all = "camelCase")]
enum DownloadEvent {
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
    /// Download id -> (yt-dlp process id, staging folder).
    running: Mutex<HashMap<String, (u32, PathBuf)>>,
    canceled: Mutex<HashSet<String>>,
}

fn format_args(req: &DownloadRequest) -> Vec<String> {
    let format = req.format.as_str();
    if req.audio_only {
        let selector = match format {
            "m4a" => "ba[ext=m4a]/ba/b",
            "opus" => "ba[acodec=opus]/ba/b",
            _ => "ba/b",
        };
        return ["-f", selector, "-x", "--audio-format", format, "--audio-quality", "0", "--embed-thumbnail"]
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

fn download_args(app: &AppHandle, req: &DownloadRequest, staging: &Path) -> Vec<String> {
    let mut args = base_args(app);
    args.extend(
        [
            "--newline",
            "--no-quiet",
            "--progress",
            "--progress-delta",
            "0.5",
            "--no-mtime",
            "--embed-metadata",
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
    args.extend([
        "-P".into(),
        staging.display().to_string(),
        "-o".into(),
        "%(title)s.%(ext)s".into(),
        "--".into(),
        req.url.clone(),
    ]);
    args
}

// Each download runs in its own hidden folder next to the destination and is moved into place
// when it finishes. yt-dlp would otherwise skip a download whose file name already exists (the same
// video in another quality, or two videos with the same title), and a canceled download leaves no
// partial files behind.

const STAGING: &str = ".ytdlp-ui";

fn staging_dir(folder: &Path, id: &str) -> Result<PathBuf, String> {
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

/// Moves `file` into `dir`, adding " (1)", " (2)"… when the name is taken, like a browser does.
fn move_unique(file: &Path, dir: &Path) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let stem = file.file_stem().unwrap_or_default().to_string_lossy();
    let ext = file.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    let mut target = dir.join(format!("{stem}{ext}"));
    let mut n = 1;
    while target.exists() {
        target = dir.join(format!("{stem} ({n}){ext}"));
        n += 1;
    }
    std::fs::rename(file, &target)?;
    Ok(target)
}

/// Removes a job's staging folder. Windows can hold files open briefly after a process is killed.
fn remove_staging(dir: &Path) {
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

    let spawned = tools::command(&ytdlp)
        .args(download_args(&app, &req, &staging))
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
            (false, true, Some(file)) => move_unique(Path::new(&file), &destination)
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

fn forward_lines(stream: Option<impl AsyncRead + Unpin + Send + 'static>, tx: mpsc::UnboundedSender<String>) {
    let Some(stream) = stream else { return };
    tauri::async_runtime::spawn(async move {
        let mut reader = BufReader::new(stream);
        let mut buf = Vec::new();
        while matches!(reader.read_until(b'\n', &mut buf).await, Ok(n) if n > 0) {
            let line = String::from_utf8_lossy(&buf).trim_end().to_string();
            buf.clear();
            if !line.is_empty() && tx.send(line).is_err() {
                break;
            }
        }
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
fn kill_tree(pid: u32, wait: bool) {
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

    #[test]
    fn moving_never_overwrites() {
        let dir = std::env::temp_dir().join(format!("ytdlp-ui-test-{}", std::process::id()));
        let (src, dest) = (dir.join("src"), dir.join("dest"));
        std::fs::create_dir_all(&src).unwrap();
        for expected in ["a.mp4", "a (1).mp4", "a (2).mp4"] {
            std::fs::write(src.join("a.mp4"), expected).unwrap();
            let moved = move_unique(&src.join("a.mp4"), &dest).unwrap();
            assert_eq!(moved.file_name().unwrap(), expected);
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn folder_names_are_safe() {
        assert_eq!(safe_folder_name(r#"Best: of "2026" / mix?"#), "Best of 2026 mix");
        assert_eq!(safe_folder_name("..."), "Playlist");
    }
}
