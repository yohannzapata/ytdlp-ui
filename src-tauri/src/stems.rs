//! Splitting a song into its parts (vocals, drums, bass, the rest) with Demucs.
//!
//! Demucs is a Python program, so it lives in a private Python environment that the app sets up only when the user
//! asks for it. `uv` builds that environment, so nothing has to be installed on the computer beforehand, and
//! nothing outside the app's own folder is touched. Everything stays on this computer: the audio is never uploaded.
//!
//! Layout inside `<app data>/stems`: `uv/` (the installer), `python/` (its Python), `venv/` (Demucs and PyTorch),
//! `models/` (the downloaded model) and `installed.json` (written last, so a half-finished install doesn't count).

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
const MODEL: &str = "htdemucs";

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

/// Runs a program to completion, reporting the last line it printed as the install detail.
async fn run_logged(
    app: &AppHandle,
    state: &StemsInstall,
    phase: &'static str,
    mut cmd: tokio::process::Command,
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
    while let Some(line) = rx.recv().await {
        if last_report.elapsed() > Duration::from_millis(120) {
            report(app, phase, line.trim(), 0, 0, None);
            last_report = Instant::now();
        }
        recent.push(line);
        if recent.len() > 6 {
            recent.remove(0);
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
    match run_logged(app, state, "python", uv_command(["python", "install", "3.12"].map(String::from).to_vec())).await {
        Err(e) if !e.contains("minor version link") => return Err(e),
        _ => {}
    }
    if let Err(e) = run_logged(app, state, "python", uv_command(venv_args())).await {
        if !e.contains("minor version link") {
            return Err(e);
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
        run_logged(app, state, "python", uv_command(venv_args())).await?;
    }
    let mut check = tools::command(&python);
    check.arg("--version");
    run_logged(app, state, "python", check).await.map_err(|_| "The private Python was installed but won't start".to_string())?;

    report(app, "packages", "Installing Demucs and PyTorch", 0, 0, None);
    run_logged(app, state, "packages", uv_command(install_args(&python, gpu))).await?;

    report(app, "model", "Downloading the separation model", 0, 0, None);
    let mut fetch_model = tools::command(&python);
    fetch_model
        .args(["-c", &format!("from demucs.pretrained import get_model; get_model('{MODEL}')")])
        .envs(envs.iter().map(|(k, v)| (*k, v.as_str())));
    run_logged(app, state, "model", fetch_model).await?;

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
// Separating a song

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeparateRequest {
    id: String,
    path: String,
    /// "two" (vocals and instrumental) or "four" (vocals, drums, bass, other).
    mode: String,
    /// "mp3" | "flac" | "wav"
    format: String,
    folder: String,
}

fn demucs_args(req: &SeparateRequest, out: &Path, input: &Path) -> Result<Vec<String>, String> {
    let mut args: Vec<String> = ["-m", "demucs", "-n", MODEL].map(String::from).to_vec();
    match req.mode.as_str() {
        "two" => args.push("--two-stems=vocals".into()),
        "four" => {}
        other => return Err(format!("Unknown separation mode: {other}")),
    }
    match req.format.as_str() {
        "mp3" => args.extend(["--mp3", "--mp3-bitrate", "320"].map(String::from)),
        "flac" => args.push("--flac".into()),
        "wav" => args.push("--int24".into()),
        other => return Err(format!("Unsupported format: {other}")),
    }
    args.extend(["-o".into(), out.display().to_string(), input.display().to_string()]);
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

/// Demucs calls the "everything but the vocals" file `no_vocals`; people look for "instrumental".
fn stem_name(file_stem: &str) -> &str {
    if file_stem == "no_vocals" { "instrumental" } else { file_stem }
}

/// Moves Demucs' results into `<parent>/<track> - stems` (or "(1)", "(2)"... if that name is taken) and returns it.
fn place_stems(out_root: &Path, parent: &Path, track: &str) -> Result<PathBuf, String> {
    let model_dir = out_root.join(MODEL);
    let produced = std::fs::read_dir(&model_dir)
        .map_err(|_| "Demucs finished but left no files".to_string())?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.is_dir())
        .ok_or("Demucs finished but left no files")?;

    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let mut target = parent.join(format!("{track} - stems"));
    let mut n = 1;
    while target.exists() {
        target = parent.join(format!("{track} - stems ({n})"));
        n += 1;
    }
    std::fs::create_dir_all(&target).map_err(|e| e.to_string())?;

    for entry in std::fs::read_dir(&produced).map_err(|e| e.to_string())?.flatten() {
        let path = entry.path();
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("stem").to_string();
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("wav");
        std::fs::rename(&path, target.join(format!("{}.{ext}", stem_name(&stem)))).map_err(|e| format!("Couldn't save the files: {e}"))?;
    }
    Ok(target)
}

#[tauri::command]
pub async fn start_separate(app: AppHandle, jobs: State<'_, Jobs>, req: SeparateRequest) -> Result<(), String> {
    let dir = stems_dir(&app)?;
    let python = python_path(&dir);
    if !python.is_file() || !marker_path(&dir).is_file() {
        return Err("Stem separation isn't installed yet. Install it in Settings.".into());
    }
    let input = PathBuf::from(&req.path);
    if !input.is_file() {
        return Err("No such file or directory".into());
    }
    let folder = PathBuf::from(&req.folder);
    if !folder.is_dir() {
        return Err("The download folder doesn't exist".into());
    }
    let track = input.file_stem().unwrap_or_default().to_string_lossy().into_owned();

    let staging = staging_dir(&folder, &req.id)?;
    let out_root = staging.join("separated");
    let args = match demucs_args(&req, &out_root, &input) {
        Ok(args) => args,
        Err(e) => {
            remove_staging(&staging);
            return Err(e);
        }
    };

    let mut cmd = tools::command(&python);
    cmd.args(&args).envs(env_for(&dir).iter().map(|(k, v)| (*k, v.as_str())));
    // Demucs can use FFmpeg to read unusual formats.
    if let Some((ffmpeg, _)) = tools::locate(&app, Tool::Ffmpeg) {
        if let Some(bin) = ffmpeg.parent() {
            let mut paths = vec![bin.to_path_buf()];
            paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()));
            if let Ok(joined) = std::env::join_paths(paths) {
                cmd.env("PATH", joined);
            }
        }
    }
    let spawned = cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true).spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(e) => {
            remove_staging(&staging);
            return Err(format!("Couldn't start Demucs: {e}"));
        }
    };

    let id = req.id;
    if let Some(pid) = child.id() {
        jobs.running.lock().unwrap().insert(id.clone(), (pid, staging.clone()));
    }
    let _ = app.emit("download", DownloadEvent::Log { id: id.clone(), line: format!("python {}", args.join(" ")) });

    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    forward_lines(child.stdout.take(), tx.clone());
    forward_lines(child.stderr.take(), tx);

    tauri::async_runtime::spawn(async move {
        let mut last_error: Option<String> = None;
        while let Some(line) = rx.recv().await {
            if let Some((percent, eta)) = parse_tqdm(&line) {
                let event = DownloadEvent::Progress {
                    id: id.clone(),
                    part: 0,
                    parts: 1,
                    percent: Some(percent),
                    downloaded: None,
                    total: None,
                    speed: None,
                    eta,
                };
                let _ = app.emit("download", event);
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
            (false, true) => place_stems(&out_root, &folder, &track).map(|p| p.display().to_string()).map_err(Some),
            (false, false) => Err(Some(last_error.unwrap_or_else(|| "Demucs stopped unexpectedly".into()))),
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

    fn request(mode: &str, format: &str) -> SeparateRequest {
        SeparateRequest { id: "1".into(), path: "in.mp3".into(), mode: mode.into(), format: format.into(), folder: String::new() }
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
    fn builds_the_demucs_command() {
        let two = demucs_args(&request("two", "mp3"), Path::new("out"), Path::new("in.mp3")).unwrap();
        assert!(two.starts_with(&["-m".into(), "demucs".into(), "-n".into(), "htdemucs".into()]));
        assert!(two.contains(&"--two-stems=vocals".to_string()) && two.contains(&"--mp3".to_string()));
        assert_eq!(&two[two.len() - 3..], ["-o", "out", "in.mp3"]);

        let four = demucs_args(&request("four", "flac"), Path::new("out"), Path::new("in.mp3")).unwrap();
        assert!(!four.iter().any(|a| a.starts_with("--two-stems")) && four.contains(&"--flac".to_string()));
        assert!(demucs_args(&request("five", "mp3"), Path::new("o"), Path::new("i")).is_err());
        assert!(demucs_args(&request("two", "ogg"), Path::new("o"), Path::new("i")).is_err());
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
    fn stems_get_friendly_names_and_a_folder_of_their_own() {
        let root = std::env::temp_dir().join(format!("ytdlp-ui-stems-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (out, parent) = (root.join("out"), root.join("music"));
        let produced = out.join(MODEL).join("Some Song");
        std::fs::create_dir_all(&produced).unwrap();
        for name in ["vocals.mp3", "no_vocals.mp3"] {
            std::fs::write(produced.join(name), name).unwrap();
        }

        let first = place_stems(&out, &parent, "Some Song").unwrap();
        assert_eq!(first, parent.join("Some Song - stems"));
        assert!(first.join("vocals.mp3").exists() && first.join("instrumental.mp3").exists());
        assert!(!first.join("no_vocals.mp3").exists());

        // A second run never touches the first result.
        let again = out.join(MODEL).join("Some Song");
        std::fs::create_dir_all(&again).unwrap();
        std::fs::write(again.join("vocals.mp3"), "v2").unwrap();
        let second = place_stems(&out, &parent, "Some Song").unwrap();
        assert_eq!(second, parent.join("Some Song - stems (1)"));
        assert_eq!(std::fs::read_to_string(first.join("vocals.mp3")).unwrap(), "vocals.mp3");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn uv_has_a_build_for_each_supported_system() {
        assert!(uv_asset().is_ok() || !matches!(std::env::consts::OS, "windows" | "macos" | "linux"));
    }
}
