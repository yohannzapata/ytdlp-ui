//! Files that are already on the computer: reading what is in them, and converting them with FFmpeg.
//!
//! A local file is just another row in the queue. Its Quality and Format choices say what comes out:
//! "Audio only · MP3" extracts the audio, "720p · MP4" shrinks the video. Jobs run in the same hidden staging
//! folder as downloads and are moved into place when they finish, so nothing is ever overwritten and canceling
//! leaves nothing behind. They report through the same `download` events, so the queue needs no special cases.

use crate::settings::DownloadOptions;
use crate::tools::{self, Tool};
use crate::ytdlp::{forward_lines, move_results, remove_staging, staging_dir, DownloadEvent, Jobs};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::mpsc;

// ---------------------------------------------------------------------------
// Reading a file

struct VideoStream {
    codec: String,
    /// The shorter side in pixels, like the qualities of downloads (1080 for 1920x1080 and for 1080x1920).
    short_side: u32,
    portrait: bool,
}

/// What FFmpeg's ffprobe found in a file.
struct Probe {
    artist: Option<String>,
    duration: Option<f64>,
    size: u64,
    video: Option<VideoStream>,
    audio_codec: Option<String>,
    /// Stream number of embedded cover art, which is a "video" stream that isn't really video.
    cover_index: Option<u32>,
    /// Stream number of the first real video stream.
    video_index: Option<u32>,
}

fn tag(v: &Value, key: &str) -> Option<String> {
    let tags = v.get("tags")?.as_object()?;
    tags.iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .and_then(|(_, value)| value.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
}

fn parse_probe(json: &Value) -> Probe {
    let mut probe = Probe {
        artist: json.get("format").and_then(|f| tag(f, "artist")),
        duration: json
            .get("format")
            .and_then(|f| f.get("duration"))
            .and_then(Value::as_str)
            .and_then(|d| d.parse().ok())
            .filter(|d: &f64| d.is_finite() && *d > 0.0),
        size: json
            .get("format")
            .and_then(|f| f.get("size"))
            .and_then(Value::as_str)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0),
        video: None,
        audio_codec: None,
        cover_index: None,
        video_index: None,
    };

    for stream in json.get("streams").and_then(Value::as_array).map_or(&[][..], Vec::as_slice) {
        let index = stream.get("index").and_then(Value::as_u64).map(|i| i as u32);
        let codec = stream.get("codec_name").and_then(Value::as_str).unwrap_or_default().to_string();
        match stream.get("codec_type").and_then(Value::as_str) {
            Some("video") => {
                let attached = stream.get("disposition").and_then(|d| d.get("attached_pic")).and_then(Value::as_i64) == Some(1);
                if attached {
                    probe.cover_index = probe.cover_index.or(index);
                } else if probe.video.is_none() {
                    let width = stream.get("width").and_then(Value::as_u64).unwrap_or(0) as u32;
                    let height = stream.get("height").and_then(Value::as_u64).unwrap_or(0) as u32;
                    probe.video = Some(VideoStream { codec, short_side: width.min(height), portrait: width < height });
                    probe.video_index = index;
                }
            }
            Some("audio") if probe.audio_codec.is_none() => probe.audio_codec = Some(codec),
            _ => {}
        }
    }
    probe
}

fn ffprobe_path(app: &AppHandle) -> Result<PathBuf, String> {
    let (ffmpeg, _) = tools::locate(app, Tool::Ffmpeg).ok_or("FFmpeg isn't installed")?;
    let name = if cfg!(windows) { "ffprobe.exe" } else { "ffprobe" };
    let path = ffmpeg.with_file_name(name);
    path.is_file().then_some(path).ok_or_else(|| "ffprobe isn't installed".into())
}

async fn probe(app: &AppHandle, path: &Path) -> Result<Probe, String> {
    if path.is_dir() {
        return Err("This is a folder. Add the files inside it instead.".into());
    }
    if !path.is_file() {
        return Err("No such file or directory".into());
    }
    let output = tools::command(&ffprobe_path(app)?)
        .args(["-v", "error", "-print_format", "json", "-show_format", "-show_streams"])
        .arg(path)
        .stdin(Stdio::null())
        .output()
        .await
        .map_err(|e| e.to_string())?;
    let json: Value = serde_json::from_slice(&output.stdout).map_err(|_| "Couldn't read this file".to_string())?;
    let probe = parse_probe(&json);
    if !output.status.success() || (probe.video.is_none() && probe.audio_codec.is_none()) {
        return Err("This doesn't look like a video or audio file".into());
    }
    Ok(probe)
}

/// A small preview picture: a frame from the video, or the cover art of a song.
async fn thumbnail(app: &AppHandle, path: &Path, probe: &Probe) -> Option<String> {
    let (ffmpeg, _) = tools::locate(app, Tool::Ffmpeg)?;
    let mut cmd = tools::command(&ffmpeg);
    cmd.args(["-hide_banner", "-loglevel", "error", "-nostdin"]);
    match (probe.video_index, probe.cover_index) {
        (Some(index), _) => {
            let at = probe.duration.map_or(0.0, |d| (d * 0.1).min(30.0));
            cmd.args(["-ss", &format!("{at:.2}")]).arg("-i").arg(path).args(["-map", &format!("0:{index}")]);
        }
        (None, Some(index)) => {
            cmd.arg("-i").arg(path).args(["-map", &format!("0:{index}")]);
        }
        (None, None) => return None,
    }
    cmd.args(["-frames:v", "1", "-vf", "scale=320:-2", "-pix_fmt", "yuvj420p", "-f", "image2pipe", "-c:v", "mjpeg", "-q:v", "6", "pipe:1"]);
    let output = cmd.stdin(Stdio::null()).output().await.ok()?;
    if output.stdout.is_empty() {
        return None;
    }
    Some(format!("data:image/jpeg;base64,{}", base64::engine::general_purpose::STANDARD.encode(output.stdout)))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaFile {
    title: String,
    artist: Option<String>,
    duration: Option<f64>,
    size: u64,
    has_video: bool,
    /// Shorter side of the video in pixels.
    height: Option<u32>,
    has_audio: bool,
    thumbnail: Option<String>,
}

#[tauri::command]
pub async fn probe_file(app: AppHandle, path: String) -> Result<MediaFile, String> {
    let path = PathBuf::from(path);
    let info = probe(&app, &path).await?;
    let thumbnail = thumbnail(&app, &path, &info).await;
    Ok(MediaFile {
        title: path.file_stem().unwrap_or_default().to_string_lossy().into_owned(),
        artist: info.artist,
        duration: info.duration,
        size: info.size,
        has_video: info.video.is_some(),
        height: info.video.as_ref().map(|v| v.short_side),
        has_audio: info.audio_codec.is_some(),
        thumbnail,
    })
}

// ---------------------------------------------------------------------------
// Converting a file

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessRequest {
    id: String,
    path: String,
    audio_only: bool,
    /// Shorter side in pixels; `None` keeps the original size.
    max_height: Option<u32>,
    /// "mp4" | "mkv" | "webm" for video, "mp3" | "m4a" | "opus" for audio.
    format: String,
    folder: String,
    options: DownloadOptions,
}

const LOUDNORM: &str = "loudnorm=I=-16:TP=-1.5:LRA=11";

fn strings<const N: usize>(items: [&str; N]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

/// The FFmpeg command for a conversion. Streams that are already in the right form are copied, not re-encoded,
/// so extracting AAC audio from a video or moving H.264 from MKV to MP4 is instant and loses nothing.
fn build_args(req: &ProcessRequest, p: &Probe, input: &Path, output: &Path) -> Result<Vec<String>, String> {
    let normalize = req.options.normalize_audio;
    let mut a = strings(["-hide_banner", "-loglevel", "warning", "-nostdin", "-y", "-i"]);
    a.push(input.display().to_string());
    a.extend(strings(["-map_metadata", "0"]));

    if req.audio_only {
        let codec = p.audio_codec.as_deref().ok_or("This file has no audio")?;
        a.extend(strings(["-map", "0:a:0"]));
        let (encode, same) = match req.format.as_str() {
            "mp3" => (strings(["-c:a", "libmp3lame", "-q:a", "0", "-id3v2_version", "3"]), codec == "mp3"),
            "m4a" => (strings(["-c:a", "aac", "-b:a", "256k"]), codec == "aac"),
            "opus" => (strings(["-c:a", "libopus", "-b:a", "160k"]), codec == "opus"),
            other => return Err(format!("Unsupported audio format: {other}")),
        };
        if same && !normalize {
            a.extend(strings(["-c:a", "copy"]));
        } else {
            a.extend(encode);
            if normalize {
                a.extend(strings(["-af", LOUDNORM]));
            }
        }
        // Songs keep their cover art (Opus files can't hold one here).
        if let (Some(cover), None, "mp3" | "m4a") = (p.cover_index, p.video_index, req.format.as_str()) {
            a.extend(["-map".into(), format!("0:{cover}")]);
            a.extend(strings(["-c:v", "copy", "-disposition:v:0", "attached_pic"]));
        }
    } else {
        let video = p.video.as_ref().ok_or("This file has no video. Choose \"Audio only\".")?;
        let format = req.format.as_str();
        if !matches!(format, "mp4" | "mkv" | "webm") {
            return Err(format!("Unsupported video format: {format}"));
        }
        a.extend(strings(["-map", "0:v:0", "-map", "0:a?"]));
        if format == "mkv" {
            a.extend(strings(["-map", "0:s?"])); // MP4 and WebM can't hold most subtitle types
        }

        let target = req.max_height.filter(|&h| video.short_side > h);
        let copy_video = target.is_none()
            && match format {
                "mp4" => matches!(video.codec.as_str(), "h264" | "hevc" | "av1"),
                "mkv" => true,
                _ => matches!(video.codec.as_str(), "vp9" | "vp8" | "av1"),
            };
        if copy_video {
            a.extend(strings(["-c:v", "copy"]));
            if format == "mp4" && video.codec == "hevc" {
                a.extend(strings(["-tag:v", "hvc1"])); // plays on Apple devices
            }
        } else {
            if let Some(h) = target {
                let scale = if video.portrait { format!("scale={h}:-2") } else { format!("scale=-2:{h}") };
                a.extend(["-vf".into(), scale]);
            }
            if format == "webm" {
                a.extend(strings(["-c:v", "libvpx-vp9", "-crf", "32", "-b:v", "0", "-row-mt", "1", "-cpu-used", "4"]));
            } else {
                a.extend(strings(["-c:v", "libx264", "-preset", "medium", "-crf", "20", "-pix_fmt", "yuv420p"]));
            }
        }

        if let Some(codec) = p.audio_codec.as_deref() {
            let copy_audio = !normalize
                && match format {
                    "mp4" => matches!(codec, "aac" | "mp3" | "ac3" | "eac3" | "opus" | "alac"),
                    "mkv" => true,
                    _ => matches!(codec, "opus" | "vorbis"),
                };
            if copy_audio {
                a.extend(strings(["-c:a", "copy"]));
            } else {
                a.extend(if format == "webm" { strings(["-c:a", "libopus", "-b:a", "128k"]) } else { strings(["-c:a", "aac", "-b:a", "192k"]) });
                if normalize {
                    a.extend(strings(["-af", LOUDNORM]));
                }
            }
        }
        if format == "mkv" {
            a.extend(strings(["-c:s", "copy"]));
        }
        if format == "mp4" {
            a.extend(strings(["-movflags", "+faststart"]));
        }
    }

    a.extend(strings(["-progress", "pipe:1", "-nostats"]));
    a.push(output.display().to_string());
    Ok(a)
}

/// Collects the `key=value` lines of `-progress` until a block ends, then reports how far along the job is.
#[derive(Default)]
struct Progress {
    done_seconds: Option<f64>,
    speed: Option<f64>,
}

impl Progress {
    /// Returns an event once per block, when FFmpeg writes its `progress=` line.
    fn feed(&mut self, id: &str, line: &str, duration: Option<f64>) -> Option<DownloadEvent> {
        let (key, value) = line.split_once('=')?;
        match key {
            "out_time_us" => self.done_seconds = value.trim().parse::<f64>().ok().map(|us| (us / 1_000_000.0).max(0.0)),
            "speed" => self.speed = value.trim().trim_end_matches('x').trim().parse().ok().filter(|s: &f64| *s > 0.0),
            "progress" => {
                let finished = value.trim() == "end";
                let percent = match (self.done_seconds, duration) {
                    _ if finished => Some(100.0),
                    (Some(done), Some(total)) => Some((done / total * 100.0).clamp(0.0, 99.9)),
                    _ => None,
                };
                let eta = match (self.done_seconds, duration, self.speed) {
                    (Some(done), Some(total), Some(speed)) if !finished => Some(((total - done) / speed).max(0.0)),
                    _ => None,
                };
                return Some(DownloadEvent::Progress {
                    id: id.to_string(),
                    part: 0,
                    parts: 1,
                    percent,
                    downloaded: None,
                    total: None,
                    speed: None,
                    eta,
                });
            }
            _ => {}
        }
        None
    }
}

fn is_progress_key(line: &str) -> bool {
    const KEYS: [&str; 11] = [
        "frame", "fps", "bitrate", "total_size", "out_time_us", "out_time_ms", "out_time", "dup_frames", "drop_frames",
        "speed", "progress",
    ];
    line.split_once('=').is_some_and(|(key, _)| KEYS.contains(&key) || key.starts_with("stream_"))
}

#[tauri::command]
pub async fn start_process(app: AppHandle, jobs: State<'_, Jobs>, req: ProcessRequest) -> Result<(), String> {
    let input = PathBuf::from(&req.path);
    let folder = PathBuf::from(&req.folder);
    if !folder.is_dir() {
        return Err("The download folder doesn't exist".into());
    }
    let info = probe(&app, &input).await?;
    let (ffmpeg, _) = tools::locate(&app, Tool::Ffmpeg).ok_or("FFmpeg isn't installed")?;

    let ext = req.format.as_str();
    let stem = input.file_stem().unwrap_or_default().to_string_lossy().into_owned();
    let staging = staging_dir(&folder, &req.id)?;
    let output = staging.join(format!("{stem}.{ext}"));
    let args = match build_args(&req, &info, &input, &output) {
        Ok(args) => args,
        Err(e) => {
            remove_staging(&staging);
            return Err(e);
        }
    };

    let spawned = tools::command(&ffmpeg)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(e) => {
            remove_staging(&staging);
            return Err(format!("Couldn't start FFmpeg: {e}"));
        }
    };

    let id = req.id;
    if let Some(pid) = child.id() {
        jobs.running.lock().unwrap().insert(id.clone(), (pid, staging.clone()));
    }
    let _ = app.emit("download", DownloadEvent::Log { id: id.clone(), line: format!("ffmpeg {}", args.join(" ")) });

    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    forward_lines(child.stdout.take(), tx.clone());
    forward_lines(child.stderr.take(), tx);

    let duration = info.duration;
    tauri::async_runtime::spawn(async move {
        let mut progress = Progress::default();
        let mut last_error: Option<String> = None;
        while let Some(line) = rx.recv().await {
            if is_progress_key(&line) {
                if let Some(event) = progress.feed(&id, &line, duration) {
                    let _ = app.emit("download", event);
                }
            } else {
                last_error = Some(line.clone());
                let _ = app.emit("download", DownloadEvent::Log { id: id.clone(), line });
            }
        }
        let success = child.wait().await.map(|s| s.success()).unwrap_or(false);

        let jobs = app.state::<Jobs>();
        jobs.running.lock().unwrap().remove(&id);
        let canceled = jobs.canceled.lock().unwrap().remove(&id);

        let result = match (canceled, success) {
            (true, _) => Err(None),
            (false, true) => move_results(&output, &staging, &folder)
                .map(|path| path.display().to_string())
                .map_err(|e| Some(format!("Couldn't save the file: {e}"))),
            (false, false) => Err(Some(last_error.unwrap_or_else(|| "FFmpeg stopped unexpectedly".into()))),
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn video_probe(codec: &str, audio: &str, short_side: u32, portrait: bool) -> Probe {
        Probe {
            artist: None,
            duration: Some(100.0),
            size: 1,
            video: Some(VideoStream { codec: codec.into(), short_side, portrait }),
            audio_codec: Some(audio.into()),
            cover_index: None,
            video_index: Some(0),
        }
    }

    fn song_probe(codec: &str, with_cover: bool) -> Probe {
        Probe {
            artist: None,
            duration: Some(200.0),
            size: 1,
            video: None,
            audio_codec: Some(codec.into()),
            cover_index: with_cover.then_some(1),
            video_index: None,
        }
    }

    fn request(audio_only: bool, max_height: Option<u32>, format: &str, normalize: bool) -> ProcessRequest {
        ProcessRequest {
            id: "1".into(),
            path: "in".into(),
            audio_only,
            max_height,
            format: format.into(),
            folder: String::new(),
            options: DownloadOptions { normalize_audio: normalize, ..DownloadOptions::default() },
        }
    }

    fn args(req: &ProcessRequest, p: &Probe) -> Vec<String> {
        build_args(req, p, Path::new("in.mkv"), Path::new("out")).unwrap()
    }

    fn has(a: &[String], pair: &[&str]) -> bool {
        a.windows(pair.len()).any(|w| w == pair)
    }

    #[test]
    fn reads_streams_and_tells_cover_art_from_video() {
        let song = parse_probe(&json!({
            "format": { "duration": "187.5", "size": "4000", "tags": { "ARTIST": "Someone" } },
            "streams": [
                { "index": 0, "codec_type": "audio", "codec_name": "mp3" },
                { "index": 1, "codec_type": "video", "codec_name": "mjpeg", "width": 500, "height": 500, "disposition": { "attached_pic": 1 } }
            ]
        }));
        assert_eq!((song.audio_codec.as_deref(), song.cover_index, song.video.is_none()), (Some("mp3"), Some(1), true));
        assert_eq!((song.duration, song.size, song.artist.as_deref()), (Some(187.5), 4000, Some("Someone")));

        let clip = parse_probe(&json!({
            "format": { "duration": "10" },
            "streams": [
                { "index": 0, "codec_type": "video", "codec_name": "h264", "width": 1080, "height": 1920, "disposition": { "attached_pic": 0 } },
                { "index": 1, "codec_type": "audio", "codec_name": "aac" }
            ]
        }));
        let video = clip.video.unwrap();
        assert_eq!((video.short_side, video.portrait, clip.video_index, clip.cover_index), (1080, true, Some(0), None));
    }

    #[test]
    fn extracting_matching_audio_is_a_lossless_copy() {
        let a = args(&request(true, None, "m4a", false), &video_probe("h264", "aac", 720, false));
        assert!(has(&a, &["-c:a", "copy"]) && has(&a, &["-map", "0:a:0"]));
        assert!(!a.contains(&"-vf".to_string()) && !a.iter().any(|x| x == "0:v:0"), "no video comes along");

        let b = args(&request(true, None, "mp3", false), &video_probe("h264", "aac", 720, false));
        assert!(has(&b, &["-c:a", "libmp3lame"]), "other formats are encoded");
    }

    #[test]
    fn songs_keep_their_cover_art_except_in_opus() {
        let mp3 = args(&request(true, None, "m4a", false), &song_probe("mp3", true));
        assert!(has(&mp3, &["-map", "0:1"]) && has(&mp3, &["-disposition:v:0", "attached_pic"]));
        let opus = args(&request(true, None, "opus", false), &song_probe("mp3", true));
        assert!(!opus.contains(&"0:1".to_string()));
        let plain = args(&request(true, None, "mp3", false), &song_probe("flac", false));
        assert!(!plain.contains(&"attached_pic".to_string()));
    }

    #[test]
    fn video_is_copied_when_possible_and_encoded_when_not() {
        let remux = args(&request(false, None, "mp4", false), &video_probe("h264", "aac", 1080, false));
        assert!(has(&remux, &["-c:v", "copy"]) && has(&remux, &["-c:a", "copy"]) && has(&remux, &["-movflags", "+faststart"]));

        let shrink = args(&request(false, Some(720), "mp4", false), &video_probe("h264", "aac", 1080, false));
        assert!(has(&shrink, &["-vf", "scale=-2:720"]) && has(&shrink, &["-c:v", "libx264"]));

        let portrait = args(&request(false, Some(720), "mp4", false), &video_probe("h264", "aac", 1080, true));
        assert!(has(&portrait, &["-vf", "scale=720:-2"]), "the short side is what's limited");

        let bigger = args(&request(false, Some(2160), "mp4", false), &video_probe("h264", "aac", 1080, false));
        assert!(has(&bigger, &["-c:v", "copy"]), "never upscales");

        let webm = args(&request(false, None, "webm", false), &video_probe("h264", "aac", 1080, false));
        assert!(has(&webm, &["-c:v", "libvpx-vp9"]) && has(&webm, &["-c:a", "libopus"]));

        let odd_audio = args(&request(false, None, "mp4", false), &video_probe("h264", "vorbis", 1080, false));
        assert!(has(&odd_audio, &["-c:v", "copy"]) && has(&odd_audio, &["-c:a", "aac"]));
    }

    #[test]
    fn volume_normalizing_forces_an_audio_encode() {
        let a = args(&request(false, None, "mp4", true), &video_probe("h264", "aac", 1080, false));
        assert!(has(&a, &["-c:v", "copy"]) && has(&a, &["-c:a", "aac"]) && a.iter().any(|x| x.starts_with("loudnorm")));
    }

    #[test]
    fn impossible_requests_are_explained() {
        let song = song_probe("mp3", false);
        let err = build_args(&request(false, None, "mp4", false), &song, Path::new("in"), Path::new("out")).unwrap_err();
        assert!(err.contains("no video"), "{err}");
        let silent = Probe { audio_codec: None, ..video_probe("h264", "aac", 720, false) };
        assert!(build_args(&request(true, None, "mp3", false), &silent, Path::new("in"), Path::new("out")).is_err());
        assert!(build_args(&request(true, None, "wav", false), &song, Path::new("in"), Path::new("out")).is_err());
    }

    #[test]
    fn progress_blocks_become_events() {
        let mut p = Progress::default();
        assert!(p.feed("1", "out_time_us=25000000", Some(100.0)).is_none());
        assert!(p.feed("1", "speed=2.5x", Some(100.0)).is_none());
        match p.feed("1", "progress=continue", Some(100.0)) {
            Some(DownloadEvent::Progress { percent, eta, .. }) => {
                assert_eq!(percent, Some(25.0));
                assert_eq!(eta, Some(30.0));
            }
            _ => panic!("expected progress"),
        }
        match p.feed("1", "progress=end", Some(100.0)) {
            Some(DownloadEvent::Progress { percent, eta, .. }) => assert_eq!((percent, eta), (Some(100.0), None)),
            _ => panic!("expected the final event"),
        }
        assert!(is_progress_key("out_time_us=1") && is_progress_key("stream_0_0_q=-1.0") && !is_progress_key("Error opening input"));
    }
}
