//! Making a file smaller: either small enough to fit a limit (Discord, email, a chat app), or as small as it can be
//! while it still looks and sounds the same.
//!
//! To fit a size, the bitrate is worked out from the size and the length, and the video is encoded twice: the first
//! pass studies the whole video, the second spends the bits where they are needed. That lands within a few percent
//! of the target, and the result is checked anyway: if it came out too big, the second pass runs again a little
//! lower. A small size for a long video would look like mush at full resolution, so the resolution (and for
//! 60 fps video the frame rate) comes down until each frame gets enough bits to look clean.
//!
//! "Same look" encodes at a constant quality instead. How much smaller that is depends on how the file was made:
//! phone and screen recordings shrink a lot, files from YouTube hardly at all. The plan says so beforehand, and a
//! result that isn't clearly smaller is not kept.

use crate::media::{self, Probe, Progress, VideoStream, LOUDNORM};
use crate::settings::DownloadOptions;
use crate::tools::{self, Tool};
use crate::ytdlp::{forward_lines, move_results, remove_staging, staging_dir, DownloadEvent, Jobs, SourceGuard};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::mpsc;

// ---------------------------------------------------------------------------
// What to aim for

#[derive(Deserialize, Serialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "camelCase", tag = "mode")]
pub enum Goal {
    /// As small as it gets while it looks (or sounds) the same.
    Same,
    /// No bigger than this many bytes.
    Size { bytes: u64 },
}

#[derive(Deserialize, Clone, Copy, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CompressChoice {
    goal: Goal,
    /// The newer video format (AV1): smaller again at the same quality, but slower to make, and older devices
    /// can't play it.
    newer_format: bool,
}

/// Room left below the target for the container and for the encoder landing a little high.
const SIZE_MARGIN: f64 = 0.975;
/// Resolutions the video can come down to, by their shorter side.
const LADDER: [u32; 7] = [2160, 1440, 1080, 720, 480, 360, 240];
/// The least bits each pixel of each frame should get for a clean picture. Below it, a smaller resolution looks
/// better on screen than a blocky bigger one. Measured with VMAF on real videos (see the tests for the numbers).
const MIN_BITS_PER_PIXEL_H264: f64 = 0.045;
/// AV1 holds up with far fewer: at 10 MB for 2.5 minutes of 1080p, 720p beat 480p by 3 VMAF points with AV1,
/// while with H.264 480p beat 720p.
const MIN_BITS_PER_PIXEL_AV1: f64 = 0.018;
/// Below this even 240p falls apart.
const MIN_VIDEO_KBPS: f64 = 90.0;
/// Constant quality for "same look". On a stand-in phone recording (1080p at 10.7 Mbps) these came out at a third
/// (H.264) and a quarter (AV1) of the size with VMAF 97, where 100 is identical and 6 points is about the least
/// anyone notices.
const SAME_LOOK_CRF_H264: u32 = 23;
const SAME_LOOK_CRF_AV1: u32 = 30;
/// A video already stored with fewer bits per pixel than this won't get much smaller at the same look.
const ALREADY_SMALL_BITS_PER_PIXEL: f64 = 0.08;
/// A "same look" result has to save at least this much to be worth keeping.
const SAME_LOOK_MIN_SAVING: f64 = 0.1;

#[derive(Serialize, Debug, PartialEq, Clone, Copy)]
#[serde(rename_all = "camelCase")]
pub enum AudioPlan {
    /// The audio is already small; it's kept as it is.
    Copy,
    /// Encoded at this many kilobits per second (mono when there's very little room).
    Encode { kbps: u32, mono: bool },
}

/// What will be done to the file, worked out before anything runs. The dialog shows it while choosing.
#[derive(Serialize, Debug, PartialEq, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    /// Shorter side of the video that comes out; `None` when there's no video.
    height: Option<u32>,
    /// The frame rate, when it comes down (60 fps to 30).
    fps: Option<f64>,
    /// Video bitrate for a size goal; `None` means constant quality.
    video_kbps: Option<u32>,
    audio: Option<AudioPlan>,
    /// About how big the result will be; `None` when that can't be known beforehand ("same look" video).
    estimate: Option<u64>,
    /// Why it can't be done, in plain words.
    problem: Option<String>,
    /// For "same look": the file is already stored about as compactly as it gets, so it won't shrink much.
    already_small: bool,
    /// The file as it is now, so the dialog can offer "half the size".
    size: u64,
    duration: Option<f64>,
}

impl Plan {
    fn refused(message: impl Into<String>) -> Plan {
        Plan { problem: Some(message.into()), ..Plan::default() }
    }
}

fn megabytes(bytes: f64) -> String {
    let mb = bytes / 1_000_000.0;
    if mb >= 10.0 { format!("{mb:.0} MB") } else { format!("{mb:.1} MB") }
}

fn bytes_for(kbps: f64, seconds: f64) -> f64 {
    kbps * 1000.0 / 8.0 * seconds
}

/// How much room the sound gets when fitting a size: less of a small budget, never more than it had.
fn audio_for_size(total_kbps: f64, p: &Probe) -> AudioPlan {
    let (kbps, mono) = match total_kbps {
        t if t >= 1500.0 => (128, false),
        t if t >= 600.0 => (96, false),
        t if t >= 300.0 => (64, false),
        _ => (40, true),
    };
    let source_kbps = p.audio_bitrate.map(|b| (b / 1000) as u32);
    let kbps = source_kbps.map_or(kbps, |source| kbps.min(source.max(32)));
    AudioPlan::Encode { kbps, mono: mono || p.audio_channels == Some(1) }
}

/// How the sound is handled for "same look": kept when it's already compact, otherwise good-quality AAC.
fn audio_for_same(p: &Probe, normalize: bool) -> AudioPlan {
    let codec = p.audio_codec.as_deref().unwrap_or_default();
    let compact = matches!(codec, "aac" | "mp3" | "opus") && p.audio_bitrate.is_some_and(|b| b <= 200_000);
    if compact && !normalize {
        AudioPlan::Copy
    } else {
        let surround = p.audio_channels.is_some_and(|c| c > 2);
        AudioPlan::Encode { kbps: if surround { 256 } else { 128 }, mono: false }
    }
}

fn audio_kbps(plan: Option<AudioPlan>, p: &Probe) -> f64 {
    match plan {
        Some(AudioPlan::Encode { kbps, .. }) => kbps as f64,
        Some(AudioPlan::Copy) => p.audio_bitrate.map_or(128.0, |b| b as f64 / 1000.0),
        None => 0.0,
    }
}

/// The highest resolution (and frame rate) at which `video_kbps` still gives a clean picture.
fn fit_picture(video_kbps: f64, short_side: u32, long_side: u32, fps: f64, newer_format: bool) -> (u32, Option<f64>) {
    let min_bpp = if newer_format { MIN_BITS_PER_PIXEL_AV1 } else { MIN_BITS_PER_PIXEL_H264 };
    let aspect = if short_side > 0 { long_side as f64 / short_side as f64 } else { 16.0 / 9.0 };
    let rungs: Vec<u32> = std::iter::once(short_side).chain(LADDER.into_iter().filter(|&h| h < short_side)).collect();
    let best = |fps: f64| {
        rungs
            .iter()
            .copied()
            .find(|&h| video_kbps * 1000.0 / (h as f64 * h as f64 * aspect * fps) >= min_bpp)
            .unwrap_or(*rungs.last().unwrap_or(&240))
    };
    let full = best(fps);
    // Smooth 60 fps matters less than a sharp picture: when the size forces the picture below 720p (or below its
    // own size, for smaller videos), the room is tight, and half the frames give each one twice the bits.
    if fps > 31.0 && full < short_side.min(720) {
        let lower = fps / (fps / 30.0).ceil();
        return (best(lower), Some(lower));
    }
    (full, None)
}

/// Bits per pixel per frame the video is stored with now, as if it were H.264 (newer formats need fewer).
fn stored_bits_per_pixel(p: &Probe) -> Option<f64> {
    let video = p.video.as_ref()?;
    let duration = p.duration?;
    let fps = video.fps.unwrap_or(30.0);
    let audio_bps = p.audio_codec.as_ref().map_or(0.0, |_| p.audio_bitrate.map_or(128_000.0, |b| b as f64));
    let video_bps = (p.size as f64 * 8.0 / duration - audio_bps).max(0.0);
    let pixels = video.short_side as f64 * video.long_side as f64 * fps;
    let worth = match video.codec.as_str() {
        "hevc" | "vp9" => 1.5,
        "av1" => 1.8,
        "h264" => 1.0,
        _ => 0.6, // older formats (MPEG-4, MJPEG, ProRes...) spend far more for the same picture
    };
    (pixels > 0.0).then(|| video_bps / pixels * worth)
}

pub fn plan(p: &Probe, choice: &CompressChoice, format: &str, normalize: bool) -> Plan {
    Plan { size: p.size, duration: p.duration, ..plan_for(p, choice, format, normalize) }
}

fn plan_for(p: &Probe, choice: &CompressChoice, format: &str, normalize: bool) -> Plan {
    let duration = p.duration.filter(|d| *d > 0.0);
    let video = p.video.as_ref().filter(|_| format == "mp4");
    if p.video.is_some() && video.is_none() && !matches!(format, "mp3" | "m4a" | "opus") {
        return Plan::refused(format!("Can't compress to {}", format.to_uppercase()));
    }

    if let Some(v) = video {
        let has_audio = p.audio_codec.is_some();
        return match choice.goal {
            Goal::Same => Plan {
                height: Some(v.short_side),
                fps: None,
                video_kbps: None,
                audio: has_audio.then(|| audio_for_same(p, normalize)),
                estimate: None,
                problem: None,
                already_small: stored_bits_per_pixel(p).is_some_and(|bpp| bpp < ALREADY_SMALL_BITS_PER_PIXEL),
                ..Plan::default()
            },
            Goal::Size { bytes } => {
                let Some(duration) = duration else { return Plan::refused("Can't tell how long this video is") };
                if bytes >= p.size {
                    return Plan::refused("It's already smaller than that");
                }
                let total_kbps = bytes as f64 * 8.0 * SIZE_MARGIN / duration / 1000.0;
                let audio_plan = has_audio.then(|| audio_for_size(total_kbps, p));
                let video_kbps = total_kbps - audio_kbps(audio_plan, p);
                if video_kbps < MIN_VIDEO_KBPS {
                    let least = bytes_for(MIN_VIDEO_KBPS + if has_audio { 40.0 } else { 0.0 }, duration) / SIZE_MARGIN;
                    return Plan::refused(format!("Too small for a video this long. The least that works is about {}", megabytes(least.ceil())));
                }
                let (height, fps) = fit_picture(video_kbps, v.short_side, v.long_side, v.fps.unwrap_or(30.0), choice.newer_format);
                Plan {
                    height: Some(height),
                    fps,
                    video_kbps: Some(video_kbps.floor() as u32),
                    estimate: Some(bytes_for(video_kbps.floor() + audio_kbps(audio_plan, p), duration).round() as u64),
                    audio: audio_plan,
                    ..Plan::default()
                }
            }
        };
    }

    // Sound only: a song, or the sound of a video saved as audio.
    if p.audio_codec.is_none() {
        return Plan::refused("This file has no audio");
    }
    let (least, most, same) = match format {
        "mp3" => (32, 320, 160),
        "m4a" => (32, 320, 160),
        "opus" => (16, 256, 128),
        other => return Plan::refused(format!("Can't compress to {}", other.to_uppercase())),
    };
    let source_kbps = p.audio_bitrate.map(|b| b as f64 / 1000.0);
    let kbps = match choice.goal {
        Goal::Same => same,
        Goal::Size { bytes } => {
            let Some(duration) = duration else { return Plan::refused("Can't tell how long this file is") };
            if bytes >= p.size {
                return Plan::refused("It's already smaller than that");
            }
            let kbps = (bytes as f64 * 8.0 * SIZE_MARGIN / duration / 1000.0).floor() as u32;
            if kbps < least {
                let smallest = bytes_for(least as f64, duration) / SIZE_MARGIN;
                return Plan::refused(format!("Too small for audio this long. The least that works is about {}", megabytes(smallest.ceil())));
            }
            kbps.min(most)
        }
    };
    Plan {
        height: None,
        fps: None,
        video_kbps: None,
        audio: Some(AudioPlan::Encode { kbps, mono: p.audio_channels == Some(1) || kbps < 48 }),
        estimate: duration.map(|d| bytes_for(kbps as f64, d).round() as u64),
        already_small: choice.goal == Goal::Same && source_kbps.is_some_and(|s| s <= same as f64 * 1.15),
        ..Plan::default()
    }
}

// ---------------------------------------------------------------------------
// The FFmpeg commands

#[derive(Clone, Copy, PartialEq, Debug)]
enum Pass {
    Only,
    First,
    Second,
}

/// What the encoder keeps between the two passes, named relative to the work folder.
const PASS_LOG: &str = "pass";

fn audio_args(audio: AudioPlan, format: &str, normalize: bool) -> Vec<String> {
    match audio {
        AudioPlan::Copy => media::strings(["-c:a", "copy"]),
        AudioPlan::Encode { kbps, mono } => {
            let mut a = match format {
                "mp3" => media::strings(["-c:a", "libmp3lame"]),
                "opus" => media::strings(["-c:a", "libopus"]),
                _ => media::strings(["-c:a", "aac"]),
            };
            a.extend(["-b:a".into(), format!("{kbps}k")]);
            if mono {
                a.extend(media::strings(["-ac", "1"]));
            }
            if normalize {
                a.extend(["-af".into(), LOUDNORM.into()]);
            }
            a
        }
    }
}

fn build_args(plan: &Plan, choice: &CompressChoice, p: &Probe, format: &str, normalize: bool, pass: Pass, input: &Path, output: &Path) -> Vec<String> {
    let mut a = media::strings(["-hide_banner", "-loglevel", "warning", "-nostdin", "-y", "-i"]);
    a.push(input.display().to_string());
    a.extend(media::strings(["-map_metadata", "0"]));

    let Some(height) = plan.height else {
        // Sound only, with the cover art of a song kept where the format can hold it.
        a.extend(media::strings(["-map", "0:a:0"]));
        if let (Some(cover), None, "mp3" | "m4a") = (p.cover_index, p.video_index, format) {
            a.extend(["-map".into(), format!("0:{cover}")]);
            a.extend(media::strings(["-c:v", "copy", "-disposition:v:0", "attached_pic"]));
        }
        if format == "mp3" {
            a.extend(media::strings(["-id3v2_version", "3"]));
        }
        a.extend(audio_args(plan.audio.unwrap_or(AudioPlan::Encode { kbps: 128, mono: false }), format, normalize));
        a.extend(media::strings(["-progress", "pipe:1", "-nostats"]));
        a.push(output.display().to_string());
        return a;
    };

    a.extend(media::strings(["-map", "0:v:0"]));
    let mut filters: Vec<String> = Vec::new();
    if let Some(fps) = plan.fps {
        filters.push(format!("fps={fps:.3}"));
    }
    if p.video.as_ref().is_some_and(|v| height < v.short_side) {
        // Sets the shorter side, whichever way the picture is turned (phones store upright video sideways).
        filters.push(format!("scale='if(gt(iw,ih),-2,{height})':'if(gt(iw,ih),{height},-2)':flags=lanczos"));
    }
    if !filters.is_empty() {
        a.extend(["-vf".into(), filters.join(",")]);
    }

    let pass_number = match pass {
        Pass::First => Some("1"),
        Pass::Second => Some("2"),
        Pass::Only => None,
    };
    if choice.newer_format {
        // Preset 8 is about as quick as x264's "medium" while AV1 still needs far fewer bits.
        a.extend(media::strings(["-c:v", "libsvtav1", "-preset", "8"]));
    } else {
        a.extend(media::strings(["-c:v", "libx264", "-preset", "medium"]));
    }
    // 8-bit 4:2:0 is what every player handles.
    a.extend(media::strings(["-pix_fmt", "yuv420p"]));
    match (plan.video_kbps, pass_number) {
        (Some(kbps), Some(n)) => {
            a.extend(["-b:v".into(), format!("{kbps}k"), "-pass".into(), n.into(), "-passlogfile".into(), PASS_LOG.into()]);
        }
        _ => {
            let crf = if choice.newer_format { SAME_LOOK_CRF_AV1 } else { SAME_LOOK_CRF_H264 };
            a.extend(["-crf".into(), crf.to_string()]);
        }
    }

    if pass == Pass::First {
        // The first pass only studies the video; nothing is written.
        a.extend(media::strings(["-an", "-progress", "pipe:1", "-nostats", "-f", "null", "-"]));
        return a;
    }
    if let Some(audio) = plan.audio {
        a.extend(media::strings(["-map", "0:a:0"]));
        a.extend(audio_args(audio, format, normalize));
    }
    a.extend(media::strings(["-movflags", "+faststart", "-progress", "pipe:1", "-nostats"]));
    a.push(output.display().to_string());
    a
}

/// The second pass again, a little lower, when the result came out `actual` bytes for a `target`.
fn lower_bitrate(video_kbps: u32, audio_kbps: f64, duration: f64, actual: u64, target: u64) -> u32 {
    let audio_bytes = bytes_for(audio_kbps, duration);
    let video_actual = (actual as f64 - audio_bytes).max(1.0);
    let video_room = (target as f64 * 0.98 - audio_bytes).max(0.0);
    ((video_kbps as f64 * video_room / video_actual).floor() as u32).max(MIN_VIDEO_KBPS as u32)
}

// ---------------------------------------------------------------------------
// Running it

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompressRequest {
    id: String,
    path: String,
    choice: CompressChoice,
    /// "mp4" for video, "mp3" | "m4a" | "opus" for sound only.
    format: String,
    folder: String,
    options: DownloadOptions,
    /// The file was downloaded only for this; it goes once the job is over.
    #[serde(default)]
    remove_input: bool,
}

/// One FFmpeg run, reporting its progress as the stretch `from..to` of the whole job.
/// `Err(None)` means the job was canceled.
async fn run_step(
    app: &AppHandle,
    id: &str,
    ffmpeg: &Path,
    args: &[String],
    work: &Path,
    staging: &Path,
    duration: Option<f64>,
    (from, to): (f64, f64),
) -> Result<(), Option<String>> {
    let jobs = app.state::<Jobs>();
    let _ = app.emit("download", DownloadEvent::Log { id: id.to_string(), line: format!("ffmpeg {}", args.join(" ")) });
    let mut child = tools::command(ffmpeg)
        .args(args)
        .current_dir(work)
        // SVT-AV1 prints a banner of settings on every run; only its warnings matter.
        .env("SVT_LOG", "2")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| Some(format!("Couldn't start FFmpeg: {e}")))?;
    if let Some(pid) = child.id() {
        jobs.running.lock().unwrap().insert(id.to_string(), (pid, staging.to_path_buf()));
    }
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    forward_lines(child.stdout.take(), tx.clone());
    forward_lines(child.stderr.take(), tx);

    let mut progress = Progress::default();
    let mut last_error: Option<String> = None;
    while let Some(line) = rx.recv().await {
        if media::is_progress_key(&line) {
            if let Some(DownloadEvent::Progress { percent, eta, .. }) = progress.feed(id, &line, duration) {
                let share = to - from;
                let event = DownloadEvent::Progress {
                    id: id.to_string(),
                    part: 0,
                    parts: 1,
                    percent: percent.map(|p| (from + p / 100.0 * share).min(99.9)),
                    downloaded: None,
                    total: None,
                    speed: None,
                    // The rest of the job takes about as long per point of progress as this step.
                    eta: eta.zip(percent).map(|(eta, p)| eta + (100.0 - to) / share * eta / (1.0 - p / 100.0).max(0.01)),
                };
                let _ = app.emit("download", event);
            }
        } else if !line.trim().is_empty() {
            last_error = Some(line.clone());
            let _ = app.emit("download", DownloadEvent::Log { id: id.to_string(), line });
        }
    }
    let success = child.wait().await.map(|s| s.success()).unwrap_or(false);
    if jobs.canceled.lock().unwrap().contains(id) {
        return Err(None);
    }
    if success { Ok(()) } else { Err(Some(last_error.unwrap_or_else(|| "FFmpeg stopped unexpectedly".into()))) }
}

struct Job {
    ffmpeg: PathBuf,
    probe: Probe,
    plan: Plan,
    choice: CompressChoice,
    format: String,
    normalize: bool,
    input: PathBuf,
    output: PathBuf,
    work: PathBuf,
    staging: PathBuf,
    folder: PathBuf,
}

async fn compress(app: &AppHandle, id: &str, job: &mut Job) -> Result<PathBuf, Option<String>> {
    let duration = job.probe.duration;
    let args = |job: &Job, pass: Pass| build_args(&job.plan, &job.choice, &job.probe, &job.format, job.normalize, pass, &job.input, &job.output);
    let step = |job: &Job, pass: Pass, share: (f64, f64)| {
        let a = args(job, pass);
        let (ffmpeg, work, staging) = (job.ffmpeg.clone(), job.work.clone(), job.staging.clone());
        async move { run_step(app, id, &ffmpeg, &a, &work, &staging, duration, share).await }
    };

    let two_pass = job.plan.video_kbps.is_some();
    if two_pass {
        // The first pass is quicker: it only studies the video.
        step(job, Pass::First, (0.0, 35.0)).await?;
        step(job, Pass::Second, (35.0, 100.0)).await?;
    } else {
        step(job, Pass::Only, (0.0, 100.0)).await?;
    }

    let size = |path: &Path| std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    match job.choice.goal {
        Goal::Size { bytes } => {
            // Two passes land within a few percent; on the rare file where they don't, try again a little lower.
            for _ in 0..2 {
                let actual = size(&job.output);
                if actual <= bytes {
                    break;
                }
                let _ = app.emit("download", DownloadEvent::Stage { id: id.to_string(), name: "FitSize".into() });
                match (job.plan.video_kbps, duration) {
                    (Some(kbps), Some(d)) => {
                        let audio = audio_kbps(job.plan.audio, &job.probe);
                        job.plan.video_kbps = Some(lower_bitrate(kbps, audio, d, actual, bytes));
                        step(job, Pass::Second, (35.0, 100.0)).await?;
                    }
                    _ => {
                        if let Some(AudioPlan::Encode { kbps, mono }) = job.plan.audio {
                            let lower = (kbps as f64 * bytes as f64 * 0.98 / actual as f64).floor() as u32;
                            job.plan.audio = Some(AudioPlan::Encode { kbps: lower.max(8), mono });
                            step(job, Pass::Only, (0.0, 100.0)).await?;
                        }
                    }
                }
            }
            let actual = size(&job.output);
            if actual > bytes {
                return Err(Some(format!("Couldn't get it under {} (it came out {})", megabytes(bytes as f64), megabytes(actual as f64))));
            }
        }
        Goal::Same => {
            let (before, after) = (job.probe.size, size(&job.output));
            if before > 0 && after as f64 > before as f64 * (1.0 - SAME_LOOK_MIN_SAVING) {
                return Err(Some(
                    "This file is already about as small as it gets at the same quality. Choose a size to make it smaller.".into(),
                ));
            }
        }
    }
    move_results(&job.output, &job.staging, &job.folder).map_err(|e| Some(format!("Couldn't save the file: {e}")))
}

/// What a plan is worked out from: a file, or a link that hasn't been downloaded yet.
#[derive(Deserialize)]
#[serde(untagged)]
pub enum PlanSource {
    File { path: String },
    /// What the site says about the video: its length, its best resolution, its frame rate and, when known, its size.
    Link { duration: f64, height: u32, fps: Option<f64>, size: Option<u64>, video: bool },
}

/// A stand-in for a file that hasn't been downloaded: sites store video compactly (VP9, AV1) with Opus sound.
fn link_probe(duration: f64, height: u32, fps: Option<f64>, size: Option<u64>, video: bool) -> Probe {
    Probe {
        duration: Some(duration),
        size: size.unwrap_or(u64::MAX),
        video: video.then(|| VideoStream {
            codec: "vp9".into(),
            short_side: height,
            long_side: height * 16 / 9,
            portrait: false,
            fps: fps.or(Some(30.0)),
        }),
        video_index: video.then_some(0),
        audio_codec: Some("opus".into()),
        audio_bitrate: Some(128_000),
        audio_channels: Some(2),
        ..Probe::default()
    }
}

/// What compressing would do, for the dialog to show while choosing.
#[tauri::command]
pub async fn compress_plan(app: AppHandle, source: PlanSource, choice: CompressChoice, format: String) -> Result<Plan, String> {
    match source {
        PlanSource::File { path } => {
            let probe = media::probe(&app, Path::new(&path)).await?;
            Ok(plan(&probe, &choice, &format, false))
        }
        PlanSource::Link { duration, height, fps, size, video } => {
            let mut planned = plan(&link_probe(duration, height, fps, size, video), &choice, &format, false);
            // Downloads from sites are already stored compactly; "same look" won't save much on them.
            planned.already_small = choice.goal == Goal::Same;
            if size.is_none() {
                planned.size = 0;
            }
            Ok(planned)
        }
    }
}

#[tauri::command]
pub async fn start_compress(app: AppHandle, req: CompressRequest) -> Result<(), String> {
    let input = PathBuf::from(&req.path);
    let source = SourceGuard(req.remove_input.then(|| input.clone()));
    let folder = PathBuf::from(&req.folder);
    if !folder.is_dir() {
        return Err("The download folder doesn't exist".into());
    }
    let probe = media::probe(&app, &input).await?;
    let normalize = req.options.normalize_audio;
    let plan = plan(&probe, &req.choice, &req.format, normalize);
    if let Some(problem) = plan.problem {
        return Err(problem);
    }
    let (ffmpeg, _) = tools::locate(&app, Tool::Ffmpeg).ok_or("FFmpeg isn't installed")?;

    let stem = input.file_stem().unwrap_or_default().to_string_lossy().into_owned();
    let staging = staging_dir(&folder, &req.id)?;
    // The encoder's notes between passes go in the system's temporary folder: never next to the result, and on a
    // short path (x264 can't open its notes in a deeply nested folder on Windows).
    let short: String = req.id.chars().filter(char::is_ascii_alphanumeric).take(8).collect();
    let work = std::env::temp_dir().join(format!("ytdlp-ui-{short}"));
    if let Err(e) = std::fs::create_dir_all(&work) {
        remove_staging(&staging);
        return Err(format!("Couldn't write to the temporary folder: {e}"));
    }
    let output = staging.join(format!("{stem} (compressed).{}", req.format));
    let id = req.id;
    let mut job = Job { ffmpeg, probe, plan, choice: req.choice, format: req.format, normalize, input, output, work, staging, folder };

    tauri::async_runtime::spawn(async move {
        let result = compress(&app, &id, &mut job).await.map(|p| p.display().to_string());
        drop(source);
        let jobs = app.state::<Jobs>();
        jobs.running.lock().unwrap().remove(&id);
        let canceled = jobs.canceled.lock().unwrap().remove(&id);
        let (staging, work) = (job.staging.clone(), job.work.clone());
        let _ = tokio::task::spawn_blocking(move || {
            remove_staging(&staging);
            let _ = std::fs::remove_dir_all(&work);
        })
        .await;
        let _ = app.emit(
            "download",
            DownloadEvent::Finished {
                ok: result.is_ok() && !canceled,
                canceled,
                filepath: result.as_ref().ok().cloned(),
                error: if canceled { None } else { result.err().flatten() },
                id,
            },
        );
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;


    fn video(short: u32, long: u32, fps: f64, codec: &str, size: u64, seconds: f64) -> Probe {
        Probe {
            duration: Some(seconds),
            size,
            video: Some(VideoStream { codec: codec.into(), short_side: short, long_side: long, fps: Some(fps), ..VideoStream::default() }),
            video_index: Some(0),
            audio_codec: Some("aac".into()),
            audio_bitrate: Some(128_000),
            audio_channels: Some(2),
            ..Probe::default()
        }
    }

    fn song(codec: &str, kbps: u64, seconds: f64) -> Probe {
        Probe {
            duration: Some(seconds),
            size: kbps * 1000 / 8 * seconds as u64,
            audio_codec: Some(codec.into()),
            audio_bitrate: Some(kbps * 1000),
            audio_channels: Some(2),
            cover_index: Some(1),
            ..Probe::default()
        }
    }

    fn size(mb: u64, newer_format: bool) -> CompressChoice {
        CompressChoice { goal: Goal::Size { bytes: mb * 1_000_000 }, newer_format }
    }

    const SAME: CompressChoice = CompressChoice { goal: Goal::Same, newer_format: false };

    fn has(a: &[String], pair: &[&str]) -> bool {
        a.windows(pair.len()).any(|w| w == pair)
    }

    #[test]
    fn a_size_goal_leaves_room_and_never_goes_over() {
        // 2.5 minutes of 1080p from YouTube, 30 MB, made to fit Discord's 10 MB.
        let p = video(1080, 1920, 24.0, "vp9", 30_196_603, 150.121);
        let plan = plan(&p, &size(10, false), "mp4", false);
        assert_eq!(plan.problem, None);
        let estimate = plan.estimate.unwrap();
        assert!(estimate <= 10_000_000 && estimate > 9_500_000, "{estimate}");
        assert_eq!(plan.audio, Some(AudioPlan::Encode { kbps: 64, mono: false }));
        assert_eq!(plan.video_kbps, Some(455));
        assert!(plan.height.unwrap() < 1080, "455 kbps can't carry a clean 1080p picture");
    }

    #[test]
    fn the_resolution_comes_down_only_as_far_as_needed() {
        let p = video(1080, 1920, 30.0, "h264", 200_000_000, 60.0);
        let roomy = plan(&p, &size(100, false), "mp4", false);
        assert_eq!(roomy.height, Some(1080), "13 Mbps keeps full HD");
        let small = plan(&p, &size(10, false), "mp4", false);
        assert!(small.height.unwrap() < roomy.height.unwrap());
        let tiny = plan(&p, &size(2, false), "mp4", false);
        assert!(tiny.height.unwrap() <= small.height.unwrap());
        // The newer format needs fewer bits for the same picture, so it can keep more pixels.
        let newer = plan(&p, &size(10, true), "mp4", false);
        assert!(newer.height.unwrap() >= small.height.unwrap());
    }

    #[test]
    fn sixty_fps_gives_way_to_a_sharper_picture_when_room_is_short() {
        let p = video(1080, 1920, 60.0, "h264", 300_000_000, 60.0);
        let roomy = plan(&p, &size(150, false), "mp4", false);
        assert_eq!((roomy.height, roomy.fps), (Some(1080), None), "with room, 60 fps stays");
        let tight = plan(&p, &size(10, false), "mp4", false);
        assert_eq!(tight.fps, Some(30.0));
        let pal = video(1080, 1920, 50.0, "h264", 300_000_000, 60.0);
        assert_eq!(plan(&pal, &size(10, false), "mp4", false).fps, Some(25.0));
    }

    #[test]
    fn impossible_sizes_are_explained() {
        let long = video(1080, 1920, 30.0, "h264", 2_000_000_000, 3600.0);
        let refused = plan(&long, &size(10, false), "mp4", false);
        let message = refused.problem.unwrap();
        assert!(message.contains("Too small") && message.contains("MB"), "{message}");

        let small = video(720, 1280, 30.0, "h264", 8_000_000, 30.0);
        assert_eq!(plan(&small, &size(10, false), "mp4", false).problem.as_deref(), Some("It's already smaller than that"));

        let unknown = Probe { duration: None, ..video(720, 1280, 30.0, "h264", 80_000_000, 30.0) };
        assert!(plan(&unknown, &size(10, false), "mp4", false).problem.is_some());
        assert!(plan(&small, &SAME, "webm", false).problem.is_some(), "only MP4 for video");
    }

    #[test]
    fn same_look_keeps_the_picture_and_warns_when_it_wont_help() {
        // A phone recording: 1080p at 17 Mbps. Lots to save.
        let phone = video(1080, 1920, 30.0, "h264", 17_000_000 / 8 * 60, 60.0);
        let plan_phone = plan(&phone, &SAME, "mp4", false);
        assert_eq!((plan_phone.height, plan_phone.fps, plan_phone.video_kbps), (Some(1080), None, None));
        assert!(!plan_phone.already_small);
        assert_eq!(plan_phone.audio, Some(AudioPlan::Copy), "128 kbps AAC is kept as it is");

        // A YouTube download: VP9 at 1.6 Mbps for 1080p is already compact.
        let youtube = video(1080, 1920, 24.0, "vp9", 30_196_603, 150.121);
        assert!(plan(&youtube, &SAME, "mp4", false).already_small);

        // Big or uncompressed sound is encoded, and volume evening always needs an encode.
        let loud = Probe { audio_codec: Some("pcm_s16le".into()), audio_bitrate: Some(1_536_000), ..phone };
        assert_eq!(plan(&loud, &SAME, "mp4", false).audio, Some(AudioPlan::Encode { kbps: 128, mono: false }));
        let normalize = video(1080, 1920, 30.0, "h264", 100_000_000, 60.0);
        assert!(matches!(plan(&normalize, &SAME, "mp4", true).audio, Some(AudioPlan::Encode { .. })));
    }

    #[test]
    fn sound_only_fits_a_size_or_keeps_its_sound() {
        let flac = song("flac", 900, 240.0); // 27 MB
        let five = plan(&flac, &size(5, false), "mp3", false);
        assert_eq!(five.audio, Some(AudioPlan::Encode { kbps: 162, mono: false }));
        assert!(five.estimate.unwrap() <= 5_000_000);
        assert_eq!(plan(&flac, &SAME, "m4a", false).audio, Some(AudioPlan::Encode { kbps: 160, mono: false }));
        assert!(!plan(&flac, &SAME, "m4a", false).already_small);
        assert!(plan(&song("mp3", 128, 240.0), &SAME, "mp3", false).already_small);
        let long = song("flac", 900, 600.0);
        assert!(plan(&long, &size(1, false), "mp3", false).problem.is_some(), "1 MB for 10 minutes is below 32 kbps");
        let huge = plan(&song("flac", 1411, 240.0), &size(40, false), "opus", false);
        assert_eq!(huge.audio, Some(AudioPlan::Encode { kbps: 256, mono: false }), "capped at what the format needs");
    }

    #[test]
    fn two_passes_share_settings_and_only_the_second_writes() {
        let p = video(1080, 1920, 60.0, "h264", 300_000_000, 60.0);
        let choice = size(10, false);
        let pl = plan(&p, &choice, "mp4", false);
        let first = build_args(&pl, &choice, &p, "mp4", false, Pass::First, Path::new("in.mov"), Path::new("out.mp4"));
        let second = build_args(&pl, &choice, &p, "mp4", false, Pass::Second, Path::new("in.mov"), Path::new("out.mp4"));
        let kbps = format!("{}k", pl.video_kbps.unwrap());
        for (a, n) in [(&first, "1"), (&second, "2")] {
            assert!(has(a, &["-c:v", "libx264"]) && has(a, &["-b:v", &kbps]) && has(a, &["-pass", n]));
            assert!(has(a, &["-passlogfile", "pass"]), "notes stay in the work folder");
            assert!(a.iter().any(|x| x.starts_with("fps=30.000,scale='if(gt(iw,ih),-2,")), "{a:?}");
        }
        assert!(has(&first, &["-f", "null"]) && first.contains(&"-an".to_string()) && !first.contains(&"out.mp4".to_string()));
        assert!(has(&second, &["-c:a", "aac"]) && has(&second, &["-movflags", "+faststart"]));
        assert_eq!(second.last().unwrap(), "out.mp4");
    }

    #[test]
    fn the_newer_format_is_av1_in_two_passes_too() {
        let p = video(1080, 1920, 30.0, "h264", 200_000_000, 60.0);
        let choice = size(10, true);
        let pl = plan(&p, &choice, "mp4", false);
        let a = build_args(&pl, &choice, &p, "mp4", false, Pass::Second, Path::new("in"), Path::new("out.mp4"));
        assert!(has(&a, &["-c:v", "libsvtav1"]) && has(&a, &["-pass", "2"]) && has(&a, &["-passlogfile", "pass"]));
        let same = CompressChoice { goal: Goal::Same, newer_format: true };
        let b = build_args(&plan(&p, &same, "mp4", false), &same, &p, "mp4", false, Pass::Only, Path::new("in"), Path::new("out.mp4"));
        assert!(has(&b, &["-c:v", "libsvtav1"]) && has(&b, &["-crf", "30"]));
    }

    #[test]
    fn same_look_is_one_constant_quality_pass_without_scaling() {
        let p = video(1080, 1920, 30.0, "h264", 200_000_000, 60.0);
        let pl = plan(&p, &SAME, "mp4", false);
        let a = build_args(&pl, &SAME, &p, "mp4", false, Pass::Only, Path::new("in"), Path::new("out.mp4"));
        assert!(has(&a, &["-crf", "23"]) && !a.contains(&"-vf".to_string()) && !a.contains(&"-pass".to_string()));
        assert!(has(&a, &["-c:a", "copy"]));
    }

    #[test]
    fn songs_keep_their_cover_and_mono_stays_mono() {
        let p = song("flac", 900, 240.0);
        let pl = plan(&p, &size(5, false), "mp3", false);
        let a = build_args(&pl, &size(5, false), &p, "mp3", false, Pass::Only, Path::new("in"), Path::new("out.mp3"));
        assert!(has(&a, &["-c:a", "libmp3lame"]) && has(&a, &["-b:a", "162k"]) && has(&a, &["-disposition:v:0", "attached_pic"]));
        let voice = Probe { audio_channels: Some(1), ..song("flac", 700, 600.0) };
        let small = plan(&voice, &size(3, false), "m4a", false);
        assert!(matches!(small.audio, Some(AudioPlan::Encode { mono: true, .. })));
    }

    #[test]
    fn a_result_that_came_out_big_is_redone_a_little_lower() {
        // 10 MB goal, came out 10.4 MB with 64 kbps sound over 150 s.
        let lower = lower_bitrate(455, 64.0, 150.0, 10_400_000, 10_000_000);
        assert!(lower < 455 && lower > 400, "{lower}");
        assert_eq!(lower_bitrate(100, 64.0, 150.0, 10_400_000, 1_000_000), MIN_VIDEO_KBPS as u32);
    }
}
