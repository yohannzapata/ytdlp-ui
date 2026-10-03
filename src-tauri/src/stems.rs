//! Splitting the sound of an audio or video file into its parts (vocals, drums, bass, guitar, piano, other) with Demucs.
//!
//! Demucs is a Python program, so it lives in a private Python environment that the app sets up only when the user
//! asks for it. `uv` builds that environment, so nothing has to be installed on the computer beforehand, and
//! nothing outside the app's own folder is touched. Everything stays on this computer: the audio is never uploaded.
//!
//! Layout inside `<app data>/stems`: `uv/` (the installer), `python/` (its Python), `venv/` (Demucs and PyTorch),
//! `models/` (the downloaded models) and `installed.json` (written last, so a half-finished install doesn't count).
//!
//! A separation runs Demucs once, which writes every part as a WAV file, and then FFmpeg writes the files the user
//! asked for: each chosen part on its own, plus (optionally) one more file that mixes all the parts that weren't chosen.

use crate::tools::{self, Tool};
use crate::ytdlp::{forward_lines, kill_tree, remove_staging, staging_dir, DownloadEvent, Jobs};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::mpsc;

/// The version of Demucs that was tested; the newer package forgets some of its own requirements, see `PACKAGES`.
const DEMUCS: &str = "demucs==4.1.0";
/// Demucs 4.1.0 imports numpy without listing it as a requirement, so it has to be installed explicitly.
const PACKAGES: [&str; 2] = [DEMUCS, "numpy"];
/// The standard model: vocals, drums, bass and "other".
const MODEL_4: &str = "htdemucs";
/// The same model with guitar and piano split out of "other" (its piano is the weakest part).
const MODEL_6: &str = "htdemucs_6s";
/// About how much the Python packages take while they download, so the install can show real progress.
/// They are measured in the package cache, which grows as the files arrive.
const PACKAGES_BYTES_CPU: u64 = 590_000_000;
const PACKAGES_BYTES_GPU: u64 = 4_900_000_000;

fn stems_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path().app_local_data_dir().map(|d| d.join("stems")).map_err(|e| e.to_string())
}

fn python_path(dir: &Path) -> PathBuf {
    if cfg!(windows) { dir.join("venv/Scripts/python.exe") } else { dir.join("venv/bin/python") }
}

fn uv_path(dir: &Path) -> PathBuf {
    dir.join("uv").join(if cfg!(windows) { "uv.exe" } else { "uv" })
}

fn marker_path(dir: &Path) -> PathBuf {
    dir.join("installed.json")
}

fn dir_size(path: &Path) -> u64 {
    std::fs::read_dir(path)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| match entry.metadata() {
            Ok(m) if m.is_dir() => dir_size(&entry.path()),
            Ok(m) => m.len(),
            Err(_) => 0,
        })
        .sum()
}

/// Environment for every Python/uv process: everything lives in the stems folder, nothing phones home.
fn env_for(dir: &Path) -> Vec<(&'static str, String)> {
    let path = |p: &str| dir.join(p).display().to_string();
    vec![
        ("UV_CACHE_DIR", path("cache")),
        ("UV_PYTHON_INSTALL_DIR", path("python")),
        ("UV_NO_PROGRESS", "1".into()),
        ("HF_HOME", path("models")),
        ("TORCH_HOME", path("models")),
        ("HF_HUB_DISABLE_TELEMETRY", "1".into()),
        ("HF_HUB_DISABLE_SYMLINKS_WARNING", "1".into()),
        ("HF_HUB_VERBOSITY", "error".into()),
        // Library warnings would otherwise show up as the install step's status line.
        ("PYTHONWARNINGS", "ignore".into()),
        ("PYTHONUTF8", "1".into()),
        ("PYTHONIOENCODING", "utf-8".into()),
    ]
}

// ---------------------------------------------------------------------------
// Status

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StemsStatus {
    installed: bool,
    /// The installed build runs on an NVIDIA GPU.
    gpu: bool,
    /// An NVIDIA GPU is present, so the faster build can be offered.
    nvidia_found: bool,
    size_bytes: u64,
}

#[derive(Serialize, Deserialize)]
struct Marker {
    gpu: bool,
}

async fn nvidia_found() -> bool {
    if cfg!(target_os = "macos") {
        return false;
    }
    match tools::command(Path::new("nvidia-smi")).arg("-L").stdin(Stdio::null()).output().await {
        Ok(out) => out.status.success() && String::from_utf8_lossy(&out.stdout).contains("GPU"),
        Err(_) => false,
    }
}

#[tauri::command]
pub async fn stems_status(app: AppHandle) -> Result<StemsStatus, String> {
    let dir = stems_dir(&app)?;
    let marker: Option<Marker> =
        std::fs::read_to_string(marker_path(&dir)).ok().and_then(|text| serde_json::from_str(&text).ok());
    let installed = marker.is_some() && python_path(&dir).is_file();
    let size_bytes = if dir.is_dir() {
        tokio::task::spawn_blocking({
            let dir = dir.clone();
            move || dir_size(&dir)
        })
        .await
        .unwrap_or(0)
    } else {
        0
    };
    Ok(StemsStatus {
        installed,
        gpu: installed && marker.is_some_and(|m| m.gpu),
        nvidia_found: nvidia_found().await,
        size_bytes,
    })
}

// ---------------------------------------------------------------------------
// Installing and removing

#[derive(Default)]
pub struct StemsInstall {
    running: AtomicBool,
    canceled: AtomicBool,
    pid: Mutex<Option<u32>>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct StemsProgress {
    /// "uv" | "python" | "packages" | "model" | "done" | "error"
    phase: &'static str,
    detail: String,
    received: u64,
    total: u64,
    error: Option<String>,
}

fn report(app: &AppHandle, phase: &'static str, detail: impl Into<String>, received: u64, total: u64, error: Option<String>) {
    let _ = app.emit("stems-progress", StemsProgress { phase, detail: detail.into(), received, total, error });
}

fn uv_asset() -> Result<&'static str, String> {
    Ok(match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => "uv-x86_64-pc-windows-msvc.zip",
        ("windows", "aarch64") => "uv-aarch64-pc-windows-msvc.zip",
        ("macos", "aarch64") => "uv-aarch64-apple-darwin.tar.gz",
        ("macos", "x86_64") => "uv-x86_64-apple-darwin.tar.gz",
        ("linux", "x86_64") => "uv-x86_64-unknown-linux-gnu.tar.gz",
        ("linux", "aarch64") => "uv-aarch64-unknown-linux-gnu.tar.gz",
        (os, arch) => return Err(format!("Stem separation isn't available on {os} {arch} yet")),
    })
}

/// How PyTorch should be chosen: the small CPU build, or whichever fits this computer's NVIDIA GPU.
fn torch_backend(gpu: bool) -> Option<&'static str> {
    match (gpu, cfg!(target_os = "macos")) {
        (_, true) => None, // the Mac build already includes everything it can use
        (true, false) => Some("auto"),
        (false, false) => Some("cpu"),
    }
}

fn install_args(python: &Path, gpu: bool) -> Vec<String> {
    let mut args: Vec<String> = ["pip", "install", "--python"].map(String::from).to_vec();
    args.push(python.display().to_string());
    args.extend(PACKAGES.map(String::from));
    if let Some(backend) = torch_backend(gpu) {
        args.extend(["--torch-backend".into(), backend.into()]);
    }
    args
}

/// Runs a program to completion, reporting the last line it printed as the install detail. With `watch` (a folder
/// and the size it is expected to reach) it also reports how full that folder is, once a second.
async fn run_logged(
    app: &AppHandle,
    state: &StemsInstall,
    phase: &'static str,
    mut cmd: tokio::process::Command,
    watch: Option<(PathBuf, u64)>,
) -> Result<(), String> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("Couldn't start the {phase} step: {e}"))?;
    *state.pid.lock().unwrap() = child.id();

    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    forward_lines(child.stdout.take(), tx.clone());
    forward_lines(child.stderr.take(), tx);

    let mut recent: Vec<String> = Vec::new();
    let mut last_report = Instant::now() - Duration::from_secs(1);
    let (mut detail, mut received) = (String::new(), 0u64);
    let total = watch.as_ref().map_or(0, |(_, expected)| *expected);
    let mut ticker = tokio::time::interval(Duration::from_secs(1));
    loop {
        tokio::select! {
            line = rx.recv() => {
                let Some(line) = line else { break };
                detail = line.trim().to_string();
                if last_report.elapsed() > Duration::from_millis(120) {
                    report(app, phase, detail.as_str(), received, total, None);
                    last_report = Instant::now();
                }
                recent.push(line);
                if recent.len() > 6 {
                    recent.remove(0);
                }
            }
            _ = ticker.tick(), if watch.is_some() => {
                if let Some((folder, _)) = watch.clone() {
                    received = tokio::task::spawn_blocking(move || dir_size(&folder)).await.unwrap_or(received);
                    report(app, phase, detail.as_str(), received, total, None);
                }
            }
        }
    }
    let status = child.wait().await.map_err(|e| e.to_string())?;
    *state.pid.lock().unwrap() = None;

    if state.canceled.load(Ordering::SeqCst) {
        Err("Canceled".into())
    } else if status.success() {
        Ok(())
    } else {
        Err(format!("The {phase} step failed: {}", recent.last().map(String::as_str).unwrap_or("no details")))
    }
}

/// Puts the `uv` program from its release archive into `<dir>/uv`.
async fn fetch_uv(app: &AppHandle, dir: &Path) -> Result<(), String> {
    let asset = uv_asset()?;
    let tmp = dir.join(".download");
    tokio::fs::create_dir_all(&tmp).await.map_err(|e| e.to_string())?;
    let archive = tmp.join(asset);
    let url = format!("https://github.com/astral-sh/uv/releases/latest/download/{asset}");
    tools::download_to(&url, &archive, |received, total| report(app, "uv", "Getting the installer", received, total, None)).await?;

    let target = uv_path(dir);
    let (archive_for_task, target_for_task, tmp_for_task) = (archive.clone(), target.clone(), tmp.clone());
    tokio::task::spawn_blocking(move || unpack_uv(&archive_for_task, &target_for_task, &tmp_for_task))
        .await
        .map_err(|e| e.to_string())??;
    let _ = tokio::fs::remove_dir_all(&tmp).await;
    Ok(())
}

fn unpack_uv(archive: &Path, target: &Path, scratch: &Path) -> Result<(), String> {
    std::fs::create_dir_all(target.parent().ok_or("bad path")?).map_err(|e| e.to_string())?;
    if archive.extension().is_some_and(|e| e == "zip") {
        let mut zip = zip::ZipArchive::new(std::fs::File::open(archive).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        for i in 0..zip.len() {
            let mut entry = zip.by_index(i).map_err(|e| e.to_string())?;
            let is_uv = entry.enclosed_name().and_then(|p| p.file_name().map(|n| n == "uv.exe")).unwrap_or(false);
            if is_uv {
                let mut out = std::fs::File::create(target).map_err(|e| e.to_string())?;
                std::io::copy(&mut entry, &mut out).map_err(|e| e.to_string())?;
                return Ok(());
            }
        }
        Err("The installer download didn't contain uv".into())
    } else {
        let unpacked = scratch.join("unpacked");
        std::fs::create_dir_all(&unpacked).map_err(|e| e.to_string())?;
        let status = std::process::Command::new("tar").arg("-xzf").arg(archive).arg("-C").arg(&unpacked).status().map_err(|e| e.to_string())?;
        if !status.success() {
            return Err("Couldn't unpack the installer".into());
        }
        let found = find_file(&unpacked, "uv").ok_or("The installer download didn't contain uv")?;
        std::fs::copy(found, target).map_err(|e| e.to_string())?;
        tools::make_executable(target)
    }
}

fn find_file(dir: &Path, name: &str) -> Option<PathBuf> {
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_file(&path, name) {
                return Some(found);
            }
        } else if path.file_name().is_some_and(|n| n == name) {
            return Some(path);
        }
    }
    None
}

async fn do_install(app: &AppHandle, state: &StemsInstall, gpu: bool) -> Result<(), String> {
    let dir = stems_dir(app)?;
    tokio::fs::create_dir_all(&dir).await.map_err(|e| e.to_string())?;
    let envs = env_for(&dir);
    let uv_command = |args: Vec<String>| {
        let mut cmd = tools::command(&uv_path(&dir));
        cmd.args(args).envs(envs.iter().map(|(k, v)| (*k, v.as_str())));
        cmd
    };
    let python = python_path(&dir);

    if !uv_path(&dir).is_file() {
        fetch_uv(app, &dir).await?;
    }

    report(app, "python", "Setting up a private Python", 0, 0, None);
    // On some computers uv unpacks Python correctly but then fails a check on its own bookkeeping ("minor version
    // link"), probably because antivirus is still scanning the new files. The install is fine; the next steps prove it.
    let venv = dir.join("venv").display().to_string();
    let venv_args = || ["venv", "--python", "3.12", "--allow-existing", &venv].map(String::from).to_vec();
    match run_logged(app, state, "python", uv_command(["python", "install", "3.12"].map(String::from).to_vec()), None).await {
        Err(e) if !e.contains("minor version link") => return Err(e),
        _ => {}
    }
    if let Err(e) = run_logged(app, state, "python", uv_command(venv_args()), None).await {
        if !e.contains("minor version link") {
            return Err(e);
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
        run_logged(app, state, "python", uv_command(venv_args()), None).await?;
    }
    let mut check = tools::command(&python);
    check.arg("--version");
    run_logged(app, state, "python", check, None).await.map_err(|_| "The private Python was installed but won't start".to_string())?;

    let expected = if gpu { PACKAGES_BYTES_GPU } else { PACKAGES_BYTES_CPU };
    report(app, "packages", "Installing Demucs and PyTorch", 0, expected, None);
    run_logged(app, state, "packages", uv_command(install_args(&python, gpu)), Some((dir.join("cache"), expected))).await?;

    report(app, "model", "Downloading the separation models", 0, 0, None);
    let mut fetch_models = tools::command(&python);
    fetch_models
        .args(["-c", &format!("from demucs.pretrained import get_model; get_model('{MODEL_4}'); get_model('{MODEL_6}')")])
        .envs(envs.iter().map(|(k, v)| (*k, v.as_str())));
    run_logged(app, state, "model", fetch_models, None).await?;

    // The package cache is only needed while installing, and can be several gigabytes.
    let _ = tokio::fs::remove_dir_all(dir.join("cache")).await;
    let marker = serde_json::to_string(&Marker { gpu }).map_err(|e| e.to_string())?;
    tokio::fs::write(marker_path(&dir), marker).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn stems_install(app: AppHandle, state: State<'_, StemsInstall>, gpu: bool) -> Result<(), String> {
    if state.running.swap(true, Ordering::SeqCst) {
        return Err("Stem separation is already being installed".into());
    }
    state.canceled.store(false, Ordering::SeqCst);
    let result = do_install(&app, &state, gpu).await;
    state.running.store(false, Ordering::SeqCst);
    // A failed install keeps what it downloaded, so trying again is quick. A canceled one leaves nothing behind.
    if result.is_err() && state.canceled.load(Ordering::SeqCst) {
        if let Ok(dir) = stems_dir(&app) {
            let _ = tokio::fs::remove_dir_all(dir).await;
        }
    }
    match &result {
        Ok(()) => report(&app, "done", "", 0, 0, None),
        Err(e) => report(&app, "error", "", 0, 0, Some(e.clone())),
    }
    result
}

#[tauri::command]
pub fn stems_cancel_install(state: State<'_, StemsInstall>) {
    state.canceled.store(true, Ordering::SeqCst);
    if let Some(pid) = *state.pid.lock().unwrap() {
        kill_tree(pid, false);
    }
}

#[tauri::command]
pub async fn stems_remove(app: AppHandle, state: State<'_, StemsInstall>) -> Result<(), String> {
    if state.running.load(Ordering::SeqCst) {
        return Err("Wait for the install to finish, or cancel it first".into());
    }
    let dir = stems_dir(&app)?;
    tokio::task::spawn_blocking(move || {
        if dir.is_dir() {
            std::fs::remove_dir_all(&dir).map_err(|e| format!("Couldn't remove it (is a separation still running?): {e}"))
        } else {
            Ok(())
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

// ---------------------------------------------------------------------------
// Separating a file

/// The parts a file can be split into, in the order they are listed.
const PARTS: [&str; 6] = ["vocals", "drums", "bass", "guitar", "piano", "other"];
/// What the standard model separates; the six-part model adds guitar and piano.
const FOUR_PARTS: [&str; 4] = ["vocals", "drums", "bass", "other"];

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeparateRequest {
    id: String,
    path: String,
    /// The parts to save as files of their own: any of `PARTS`.
    parts: Vec<String>,
    /// Also save everything that wasn't chosen, mixed together, as one more file.
    rest: bool,
    /// "mp3" | "flac" | "wav"
    format: String,
    folder: String,
}

/// One file to write: one part, or the mix of several.
#[derive(Debug, PartialEq)]
struct Output {
    name: String,
    parts: Vec<&'static str>,
}

#[derive(Debug, PartialEq)]
struct Plan {
    model: &'static str,
    outputs: Vec<Output>,
}

/// Works out which model to run and which files to write. Guitar and piano need the six-part model; otherwise the
/// standard one is used, which is more accurate on the parts they share.
fn plan(parts: &[String], rest: bool) -> Result<Plan, String> {
    if let Some(unknown) = parts.iter().find(|p| !PARTS.contains(&p.as_str())) {
        return Err(format!("Unknown part: {unknown}"));
    }
    let chosen: Vec<&'static str> = PARTS.iter().copied().filter(|p| parts.iter().any(|c| c == p)).collect();
    if chosen.is_empty() {
        return Err("Choose at least one part to save".into());
    }
    let six = chosen.iter().any(|p| matches!(*p, "guitar" | "piano"));
    let (model, available): (&'static str, &[&'static str]) = if six { (MODEL_6, &PARTS[..]) } else { (MODEL_4, &FOUR_PARTS[..]) };

    let mut outputs: Vec<Output> = chosen.iter().map(|p| Output { name: (*p).to_string(), parts: vec![*p] }).collect();
    let left: Vec<&'static str> = available.iter().copied().filter(|p| !chosen.contains(p)).collect();
    if rest && !left.is_empty() {
        let name = match chosen.as_slice() {
            ["vocals"] => "instrumental".to_string(),
            [one] if *one != "other" => format!("no {one}"),
            _ => "everything else".to_string(),
        };
        outputs.push(Output { name, parts: left });
    }
    Ok(Plan { model, outputs })
}

/// Demucs writes every part as a float WAV, so nothing is lost before FFmpeg makes the final files.
fn demucs_args(model: &str, out: &Path, input: &Path) -> Vec<String> {
    let mut args: Vec<String> = ["-m", "demucs", "-n", model, "--float32", "--filename", "{stem}.{ext}", "-o"].map(String::from).to_vec();
    args.extend([out.display().to_string(), input.display().to_string()]);
    args
}

/// Keeps a mix of several parts from going over full scale (which would crackle), without changing anything below it.
const LIMITER: &str = "alimiter=limit=0.98:level=false:latency=true";

/// The FFmpeg command that writes one output: a single part, or the sum of several, in the chosen format.
fn ffmpeg_args(output: &Output, parts_dir: &Path, format: &str, target: &Path) -> Result<Vec<String>, String> {
    let mut args: Vec<String> = ["-hide_banner", "-loglevel", "error", "-nostdin", "-y"].map(String::from).to_vec();
    for part in &output.parts {
        args.extend(["-i".into(), parts_dir.join(format!("{part}.wav")).display().to_string()]);
    }
    if output.parts.len() > 1 {
        let inputs: String = (0..output.parts.len()).map(|i| format!("[{i}:a]")).collect();
        let graph = format!("{inputs}amix=inputs={}:normalize=0:duration=longest,{LIMITER}[mix]", output.parts.len());
        args.extend(["-filter_complex".into(), graph, "-map".into(), "[mix]".into()]);
    } else {
        args.extend(["-af".into(), LIMITER.into()]);
    }
    let codec: &[&str] = match format {
        "mp3" => &["-c:a", "libmp3lame", "-b:a", "320k"],
        "flac" => &["-c:a", "flac", "-sample_fmt", "s32", "-bits_per_raw_sample", "24"],
        "wav" => &["-c:a", "pcm_s24le"],
        other => return Err(format!("Unsupported format: {other}")),
    };
    args.extend(codec.iter().map(|s| s.to_string()));
    args.push(target.display().to_string());
    Ok(args)
}

/// Reads tqdm's progress line, such as `  37%|###   | 66.3/179.4 [00:30<00:50, 2.2seconds/s]`:
/// the percentage, and the seconds left when it shows them.
fn parse_tqdm(line: &str) -> Option<(f64, Option<f64>)> {
    let bar = line.find("%|")?;
    let digits: String = line[..bar].chars().rev().take_while(char::is_ascii_digit).collect::<Vec<_>>().into_iter().rev().collect();
    let percent: f64 = digits.parse().ok()?;
    let eta = line.split_once('<').and_then(|(_, rest)| {
        let clock = rest.split([',', ']']).next()?.trim();
        let parts: Vec<f64> = clock.split(':').map(|p| p.parse().ok()).collect::<Option<_>>()?;
        Some(parts.iter().fold(0.0, |total, part| total * 60.0 + part))
    });
    Some((percent.min(100.0), eta))
}

/// Moves the finished files into `<parent>/<track> - stems` (or "(1)", "(2)"... if that name is taken) and returns it.
fn place_stems(finished: &Path, parent: &Path, track: &str) -> Result<PathBuf, String> {
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let mut target = parent.join(format!("{track} - stems"));
    let mut n = 1;
    while target.exists() {
        target = parent.join(format!("{track} - stems ({n})"));
        n += 1;
    }
    std::fs::create_dir_all(&target).map_err(|e| e.to_string())?;
    let mut moved = 0;
    for entry in std::fs::read_dir(finished).map_err(|_| "The separation finished but left no files".to_string())?.flatten() {
        std::fs::rename(entry.path(), target.join(entry.file_name())).map_err(|e| format!("Couldn't save the files: {e}"))?;
        moved += 1;
    }
    if moved == 0 {
        let _ = std::fs::remove_dir(&target);
        return Err("The separation finished but left no files".into());
    }
    Ok(target)
}

fn emit_progress(app: &AppHandle, id: &str, percent: f64, eta: Option<f64>) {
    let event = DownloadEvent::Progress {
        id: id.to_string(),
        part: 0,
        parts: 1,
        percent: Some(percent),
        downloaded: None,
        total: None,
        speed: None,
        eta,
    };
    let _ = app.emit("download", event);
}

/// Everything a separation needs once it is running.
struct Job {
    ffmpeg: PathBuf,
    /// Demucs, ready to start once FFmpeg has read the file.
    demucs: tokio::process::Command,
    plan: Plan,
    /// Where Demucs leaves the parts.
    parts_dir: PathBuf,
    format: String,
    staging: PathBuf,
    folder: PathBuf,
    track: String,
}

/// Demucs reads audio files itself, but it keeps the silence that MP3 encoders add at the start, which would leave
/// every part about 25 ms behind the original. FFmpeg removes that silence, and also opens video files and picks the
/// first audio track, so Demucs is given plain stereo WAV at the sample rate its models use.
fn read_args(input: &Path, wav: &Path) -> Vec<String> {
    let mut args: Vec<String> = ["-hide_banner", "-loglevel", "error", "-nostdin", "-y", "-i"].map(String::from).to_vec();
    args.push(input.display().to_string());
    args.extend(["-map", "0:a:0", "-vn", "-ac", "2", "-ar", "44100", "-c:a", "pcm_f32le"].map(String::from));
    args.push(wav.display().to_string());
    args
}

/// The last thing a program said on its error output, for telling the user what went wrong.
fn last_message(stderr: &[u8], fallback: &str) -> String {
    String::from_utf8_lossy(stderr).lines().rev().find(|l| !l.trim().is_empty()).unwrap_or(fallback).trim().to_string()
}

/// Reads the file, runs Demucs, then writes the files. `Err(None)` means the job was canceled.
async fn separate(app: &AppHandle, id: &str, reading: tokio::process::Child, mut job: Job) -> Result<PathBuf, Option<String>> {
    let jobs = app.state::<Jobs>();
    let canceled = || jobs.canceled.lock().unwrap().contains(id);

    let _ = app.emit("download", DownloadEvent::Stage { id: id.to_string(), name: "ReadFile".into() });
    let read = reading.wait_with_output().await.map_err(|e| Some(e.to_string()))?;
    if canceled() {
        return Err(None);
    }
    if !read.status.success() {
        let message = last_message(&read.stderr, "FFmpeg couldn't read the file");
        return Err(Some(if message.contains("matches no streams") { "This file has no audio".into() } else { message }));
    }

    let spawned = job.demucs.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true).spawn();
    let mut child = spawned.map_err(|e| Some(format!("Couldn't start Demucs: {e}")))?;
    if let Some(pid) = child.id() {
        jobs.running.lock().unwrap().insert(id.to_string(), (pid, job.staging.clone()));
    }
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    forward_lines(child.stdout.take(), tx.clone());
    forward_lines(child.stderr.take(), tx);

    let mut last_error: Option<String> = None;
    while let Some(line) = rx.recv().await {
        if let Some((percent, eta)) = parse_tqdm(&line) {
            // Demucs is most of the work; writing the files takes the last tenth.
            emit_progress(app, id, percent * 0.9, eta);
        } else {
            last_error = Some(line.clone());
            let _ = app.emit("download", DownloadEvent::Log { id: id.to_string(), line });
        }
    }
    let success = child.wait().await.map(|s| s.success()).unwrap_or(false);
    if canceled() {
        return Err(None);
    }
    if !success {
        return Err(Some(last_error.unwrap_or_else(|| "Demucs stopped unexpectedly".into())));
    }
    write_files(app, id, &job).await
}

/// After Demucs has written the parts: FFmpeg makes each file, then they are put in place.
async fn write_files(app: &AppHandle, id: &str, job: &Job) -> Result<PathBuf, Option<String>> {
    let jobs = app.state::<Jobs>();
    let canceled = || jobs.canceled.lock().unwrap().contains(id);
    let _ = app.emit("download", DownloadEvent::Stage { id: id.to_string(), name: "SaveStems".into() });

    let ready = job.staging.join("ready");
    std::fs::create_dir_all(&ready).map_err(|e| Some(e.to_string()))?;
    let count = job.plan.outputs.len();
    for (i, output) in job.plan.outputs.iter().enumerate() {
        if canceled() {
            return Err(None);
        }
        let target = ready.join(format!("{}.{}", output.name, job.format));
        let args = ffmpeg_args(output, &job.parts_dir, &job.format, &target).map_err(Some)?;
        let mut cmd = tools::command(&job.ffmpeg);
        cmd.args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).kill_on_drop(true);
        let child = cmd.spawn().map_err(|e| Some(format!("Couldn't start FFmpeg: {e}")))?;
        if let Some(pid) = child.id() {
            jobs.running.lock().unwrap().insert(id.to_string(), (pid, job.staging.clone()));
        }
        let done = child.wait_with_output().await;
        if canceled() {
            return Err(None);
        }
        match done {
            Ok(out) if out.status.success() => {}
            Ok(out) => {
                let reason = last_message(&out.stderr, "FFmpeg stopped unexpectedly");
                return Err(Some(format!("Couldn't save \"{}\": {reason}", output.name)));
            }
            Err(e) => return Err(Some(e.to_string())),
        }
        emit_progress(app, id, 90.0 + 10.0 * (i + 1) as f64 / count as f64, None);
    }
    place_stems(&ready, &job.folder, &job.track).map_err(Some)
}

#[tauri::command]
pub async fn start_separate(app: AppHandle, jobs: State<'_, Jobs>, req: SeparateRequest) -> Result<(), String> {
    let dir = stems_dir(&app)?;
    let python = python_path(&dir);
    if !python.is_file() || !marker_path(&dir).is_file() {
        return Err("Audio separation isn't installed yet. Install it in Settings.".into());
    }
    let input = PathBuf::from(&req.path);
    if !input.is_file() {
        return Err("No such file or directory".into());
    }
    let folder = PathBuf::from(&req.folder);
    if !folder.is_dir() {
        return Err("The download folder doesn't exist".into());
    }
    let plan = plan(&req.parts, req.rest)?;
    let (ffmpeg, _) = tools::locate(&app, Tool::Ffmpeg).ok_or("FFmpeg isn't available")?;
    let track = input.file_stem().unwrap_or_default().to_string_lossy().into_owned();

    let staging = staging_dir(&folder, &req.id)?;
    let wav = staging.join("input.wav");
    let out_root = staging.join("separated");

    let mut demucs = tools::command(&python);
    demucs.args(demucs_args(plan.model, &out_root, &wav)).envs(env_for(&dir).iter().map(|(k, v)| (*k, v.as_str())));
    // Demucs may still call FFmpeg for the WAV file, so it should find the app's own copy.
    if let Some(bin) = ffmpeg.parent() {
        let mut paths = vec![bin.to_path_buf()];
        paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()));
        if let Ok(joined) = std::env::join_paths(paths) {
            demucs.env("PATH", joined);
        }
    }

    // The first step starts now, so the job can be canceled from the moment this returns.
    let mut read = tools::command(&ffmpeg);
    read.args(read_args(&input, &wav)).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).kill_on_drop(true);
    let reading = match read.spawn() {
        Ok(child) => child,
        Err(e) => {
            remove_staging(&staging);
            return Err(format!("Couldn't start FFmpeg: {e}"));
        }
    };
    let id = req.id;
    if let Some(pid) = reading.id() {
        jobs.running.lock().unwrap().insert(id.clone(), (pid, staging.clone()));
    }
    let _ = app.emit(
        "download",
        DownloadEvent::Log { id: id.clone(), line: format!("python {}", demucs_args(plan.model, &out_root, &wav).join(" ")) },
    );

    let job = Job { ffmpeg, demucs, parts_dir: out_root.join(plan.model), plan, format: req.format, staging: staging.clone(), folder, track };
    tauri::async_runtime::spawn(async move {
        let result = separate(&app, &id, reading, job).await.map(|p| p.display().to_string());

        let jobs = app.state::<Jobs>();
        jobs.running.lock().unwrap().remove(&id);
        let canceled = jobs.canceled.lock().unwrap().remove(&id);
        let _ = tokio::task::spawn_blocking(move || remove_staging(&staging)).await;

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

    fn names(plan: &Plan) -> Vec<(&str, Vec<&str>)> {
        plan.outputs.iter().map(|o| (o.name.as_str(), o.parts.clone())).collect()
    }
    fn chosen(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|p| p.to_string()).collect()
    }

    #[test]
    fn reads_tqdm_progress() {
        assert_eq!(parse_tqdm("  37%|###       | 66.30/179.4 [00:30<00:50,  2.2seconds/s]"), Some((37.0, Some(50.0))));
        assert_eq!(parse_tqdm("100%|##########| 181.35/181.35 [00:12<00:00, 14.18seconds/s]"), Some((100.0, Some(0.0))));
        assert_eq!(parse_tqdm("  5%|#         | 9/180 [00:01<1:02:03, 1.0seconds/s]"), Some((5.0, Some(3723.0))));
        assert_eq!(parse_tqdm("Selected model is a bag of 1 models."), None);
        assert_eq!(parse_tqdm("Separated tracks will be stored in out"), None);
    }

    #[test]
    fn vocals_with_the_rest_is_the_classic_karaoke_pair() {
        let p = plan(&chosen(&["vocals"]), true).unwrap();
        assert_eq!(p.model, "htdemucs");
        assert_eq!(names(&p), [("vocals", vec!["vocals"]), ("instrumental", vec!["drums", "bass", "other"])]);
        assert_eq!(names(&plan(&chosen(&["vocals"]), false).unwrap()), [("vocals", vec!["vocals"])]);
    }

    #[test]
    fn guitar_and_piano_use_the_six_part_model() {
        let p = plan(&chosen(&["guitar", "drums"]), true).unwrap();
        assert_eq!(p.model, "htdemucs_6s");
        // Parts come out in the usual order, whatever order they were chosen in, and the rest mixes what is left.
        assert_eq!(
            names(&p),
            [("drums", vec!["drums"]), ("guitar", vec!["guitar"]), ("everything else", vec!["vocals", "bass", "piano", "other"])]
        );
        assert_eq!(plan(&chosen(&["piano"]), false).unwrap().model, "htdemucs_6s");
    }

    #[test]
    fn the_rest_is_named_for_what_it_leaves_out() {
        assert_eq!(plan(&chosen(&["drums"]), true).unwrap().outputs[1].name, "no drums");
        assert_eq!(plan(&chosen(&["other"]), true).unwrap().outputs[1].name, "everything else");
        assert_eq!(plan(&chosen(&["vocals", "bass"]), true).unwrap().outputs[2].name, "everything else");
    }

    #[test]
    fn there_is_no_rest_file_when_every_part_was_chosen() {
        let four = plan(&chosen(&["vocals", "drums", "bass", "other"]), true).unwrap();
        assert_eq!((four.model, four.outputs.len()), ("htdemucs", 4));
        let six = plan(&chosen(&PARTS), true).unwrap();
        assert_eq!((six.model, six.outputs.len()), ("htdemucs_6s", 6));
        // With guitar chosen, piano is still left over.
        let almost = plan(&chosen(&["vocals", "drums", "bass", "other", "guitar"]), true).unwrap();
        assert_eq!(names(&almost).last(), Some(&("everything else", vec!["piano"])));
    }

    #[test]
    fn bad_choices_are_refused() {
        assert!(plan(&[], true).is_err());
        assert!(plan(&chosen(&["kazoo"]), true).is_err());
        // The same part twice is one part.
        assert_eq!(plan(&chosen(&["vocals", "vocals"]), false).unwrap().outputs.len(), 1);
    }

    #[test]
    fn builds_the_demucs_command() {
        let args = demucs_args("htdemucs_6s", Path::new("out"), Path::new("in.mp3"));
        assert!(args.starts_with(&["-m".into(), "demucs".into(), "-n".into(), "htdemucs_6s".into()]));
        assert!(args.contains(&"--float32".to_string()));
        let at = args.iter().position(|a| a == "--filename").unwrap();
        assert_eq!(args[at + 1], "{stem}.{ext}", "files land directly in the model's folder");
        assert_eq!(&args[args.len() - 3..], ["-o", "out", "in.mp3"]);
    }

    #[test]
    fn demucs_is_given_plain_wav_that_ffmpeg_prepared() {
        let args = read_args(Path::new("clip.mp4"), Path::new("input.wav"));
        assert!(args.windows(2).any(|w| w == ["-map", "0:a:0"]), "the first audio track is used");
        assert!(args.contains(&"-vn".to_string()), "video is dropped");
        assert!(args.windows(2).any(|w| w == ["-ar", "44100"]) && args.windows(2).any(|w| w == ["-ac", "2"]));
        assert!(args.windows(2).any(|w| w == ["-c:a", "pcm_f32le"]), "nothing is lost on the way to Demucs");
        assert_eq!(args.last().unwrap(), "input.wav");
        assert_eq!(last_message(b"first\nStream map '0:a:0' matches no streams.\n\n", "x"), "Stream map '0:a:0' matches no streams.");
        assert_eq!(last_message(b"", "fallback"), "fallback");
    }

    #[test]
    fn a_single_part_is_written_as_it_is() {
        let one = Output { name: "vocals".into(), parts: vec!["vocals"] };
        let args = ffmpeg_args(&one, Path::new("parts"), "mp3", Path::new("vocals.mp3")).unwrap();
        assert_eq!(args.iter().filter(|a| *a == "-i").count(), 1);
        assert!(!args.contains(&"-filter_complex".to_string()) && args.contains(&"-af".to_string()));
        assert!(args.contains(&"libmp3lame".to_string()) && args.contains(&"320k".to_string()));
        assert_eq!(args.last().unwrap(), "vocals.mp3");
    }

    #[test]
    fn several_parts_are_mixed_at_their_original_level() {
        let rest = Output { name: "instrumental".into(), parts: vec!["drums", "bass", "other"] };
        let args = ffmpeg_args(&rest, Path::new("parts"), "flac", Path::new("instrumental.flac")).unwrap();
        assert_eq!(args.iter().filter(|a| *a == "-i").count(), 3);
        let graph = &args[args.iter().position(|a| a == "-filter_complex").unwrap() + 1];
        assert!(graph.starts_with("[0:a][1:a][2:a]amix=inputs=3:normalize=0"), "{graph}");
        assert!(graph.contains("alimiter") && graph.ends_with("[mix]"));
        assert!(args.windows(2).any(|w| w == ["-map", "[mix]"]));
        assert!(args.windows(2).any(|w| w == ["-sample_fmt", "s32"]), "FLAC is written with 24 bits");
    }

    #[test]
    fn each_format_has_its_encoder() {
        let one = Output { name: "bass".into(), parts: vec!["bass"] };
        let wav = ffmpeg_args(&one, Path::new("p"), "wav", Path::new("bass.wav")).unwrap();
        assert!(wav.contains(&"pcm_s24le".to_string()));
        assert!(ffmpeg_args(&one, Path::new("p"), "ogg", Path::new("bass.ogg")).is_err());
    }

    #[test]
    fn the_install_uses_the_right_pytorch() {
        let python = Path::new("py");
        let cpu = install_args(python, false);
        assert!(cpu.contains(&DEMUCS.to_string()) && cpu.contains(&"numpy".to_string()), "numpy is requested explicitly");
        let gpu = install_args(python, true);
        if cfg!(target_os = "macos") {
            assert!(!cpu.contains(&"--torch-backend".to_string()));
        } else {
            assert_eq!(&cpu[cpu.len() - 2..], ["--torch-backend", "cpu"]);
            assert_eq!(&gpu[gpu.len() - 2..], ["--torch-backend", "auto"]);
        }
    }

    #[test]
    fn the_files_get_a_folder_of_their_own() {
        let root = std::env::temp_dir().join(format!("ytdlp-ui-stems-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (ready, parent) = (root.join("ready"), root.join("music"));
        std::fs::create_dir_all(&ready).unwrap();
        for name in ["vocals.mp3", "instrumental.mp3"] {
            std::fs::write(ready.join(name), name).unwrap();
        }

        let first = place_stems(&ready, &parent, "Some Song").unwrap();
        assert_eq!(first, parent.join("Some Song - stems"));
        assert!(first.join("vocals.mp3").exists() && first.join("instrumental.mp3").exists());

        // A second run never touches the first result.
        std::fs::write(ready.join("vocals.mp3"), "v2").unwrap();
        let second = place_stems(&ready, &parent, "Some Song").unwrap();
        assert_eq!(second, parent.join("Some Song - stems (1)"));
        assert_eq!(std::fs::read_to_string(first.join("vocals.mp3")).unwrap(), "vocals.mp3");

        // Nothing to place is an error, and leaves no empty folder behind.
        assert!(place_stems(&ready, &parent, "Some Song").is_err());
        assert!(!parent.join("Some Song - stems (2)").exists());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn uv_has_a_build_for_each_supported_system() {
        assert!(uv_asset().is_ok() || !matches!(std::env::consts::OS, "windows" | "macos" | "linux"));
    }
}
