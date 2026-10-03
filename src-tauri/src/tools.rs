//! The external programs the app relies on: yt-dlp, ffmpeg and deno.
//!
//! Each one is downloaded into the app's own `bin` folder on first run. ffmpeg and
//! deno already installed on the system are used as-is; yt-dlp is always our own
//! copy so it can update itself with `yt-dlp -U`.

use futures_util::{future::join_all, StreamExt};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::AsyncWriteExt;

const EXE_SUFFIX: &str = if cfg!(windows) { ".exe" } else { "" };

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tool {
    Ytdlp,
    Ffmpeg,
    Deno,
}

impl Tool {
    const ALL: [Tool; 3] = [Tool::Ytdlp, Tool::Ffmpeg, Tool::Deno];

    fn file_name(self) -> String {
        let base = match self {
            Tool::Ytdlp => "yt-dlp",
            Tool::Ffmpeg => "ffmpeg",
            Tool::Deno => "deno",
        };
        format!("{base}{EXE_SUFFIX}")
    }
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    App,
    System,
}

/// A command that never flashes a console window on Windows.
pub fn command(program: &Path) -> tokio::process::Command {
    #[allow(unused_mut)]
    let mut cmd = tokio::process::Command::new(program);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    cmd
}

pub fn bin_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_local_data_dir()
        .map(|dir| dir.join("bin"))
        .map_err(|e| e.to_string())
}

/// Finds a tool: our own copy first, then (except yt-dlp) the system's.
pub fn locate(app: &AppHandle, tool: Tool) -> Option<(PathBuf, Source)> {
    let own = bin_dir(app).ok()?.join(tool.file_name());
    if own.is_file() {
        return Some((own, Source::App));
    }
    if tool == Tool::Ytdlp {
        return None;
    }
    find_on_system(&tool.file_name()).map(|path| (path, Source::System))
}

fn find_on_system(file_name: &str) -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    // Apps started from the macOS Dock don't get the shell's PATH.
    dirs.extend(["/opt/homebrew/bin", "/usr/local/bin"].map(PathBuf::from));
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(home).join(".deno/bin"));
    }
    dirs.into_iter()
        .map(|dir| dir.join(file_name))
        .find(|path| path.is_file())
}

pub async fn version(tool: Tool, path: &Path) -> Option<String> {
    let flag = if tool == Tool::Ffmpeg { "-version" } else { "--version" };
    let output = command(path).arg(flag).output().await.ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let first_line = text.lines().next()?.trim();
    let version = match tool {
        Tool::Ytdlp => first_line,                              // "2026.08.19"
        Tool::Ffmpeg => first_line.split_whitespace().nth(2)?,  // "ffmpeg version N-12345-g... Copyright"
        Tool::Deno => first_line.split_whitespace().nth(1)?,    // "deno 2.9.7 (stable, ...)"
    };
    Some(version.to_string())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolStatus {
    tool: Tool,
    found: bool,
    source: Option<Source>,
    version: Option<String>,
}

#[tauri::command]
pub async fn tools_status(app: AppHandle) -> Vec<ToolStatus> {
    join_all(Tool::ALL.map(|tool| {
        let found = locate(&app, tool);
        async move {
            match found {
                Some((path, source)) => ToolStatus {
                    tool,
                    found: true,
                    source: Some(source),
                    version: version(tool, &path).await,
                },
                None => ToolStatus { tool, found: false, source: None, version: None },
            }
        }
    }))
    .await
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct InstallProgress {
    tool: Tool,
    /// "downloading" | "extracting" | "done" | "error"
    phase: &'static str,
    received: u64,
    total: u64,
    error: Option<String>,
}

fn report(app: &AppHandle, tool: Tool, phase: &'static str, received: u64, total: u64, error: Option<String>) {
    let _ = app.emit("tools-progress", InstallProgress { tool, phase, received, total, error });
}

/// Downloads every missing tool at the same time.
#[tauri::command]
pub async fn tools_install(app: AppHandle) -> Result<(), String> {
    let bin = bin_dir(&app)?;
    let tmp = bin.join(".tmp");
    tokio::fs::create_dir_all(&tmp).await.map_err(|e| e.to_string())?;

    let missing = Tool::ALL.into_iter().filter(|&tool| locate(&app, tool).is_none());
    let results = join_all(missing.map(|tool| {
        let (app, bin, tmp) = (app.clone(), bin.clone(), tmp.clone());
        async move {
            let result = install(&app, tool, &bin, &tmp).await;
            match &result {
                Ok(()) => report(&app, tool, "done", 0, 0, None),
                Err(e) => report(&app, tool, "error", 0, 0, Some(e.clone())),
            }
            result
        }
    }))
    .await;

    let _ = tokio::fs::remove_dir_all(&tmp).await;
    results.into_iter().collect()
}

async fn install(app: &AppHandle, tool: Tool, bin: &Path, tmp: &Path) -> Result<(), String> {
    match tool {
        Tool::Ytdlp => {
            let url = format!("https://github.com/yt-dlp/yt-dlp/releases/latest/download/{}", ytdlp_asset()?);
            let file = tmp.join(tool.file_name());
            download(app, tool, &url, &file).await?;
            make_executable(&file)?;
            std::fs::rename(&file, bin.join(tool.file_name())).map_err(|e| e.to_string())
        }
        Tool::Deno => {
            let url = format!("https://github.com/denoland/deno/releases/latest/download/deno-{}.zip", deno_target()?);
            let archive = tmp.join("deno.zip");
            download(app, tool, &url, &archive).await?;
            report(app, tool, "extracting", 0, 0, None);
            extract(archive, bin.to_path_buf()).await
        }
        Tool::Ffmpeg => {
            for (i, url) in ffmpeg_urls()?.iter().enumerate() {
                let name = url.rsplit('/').next().unwrap_or("ffmpeg.zip");
                let archive = tmp.join(format!("{i}-{name}"));
                download(app, tool, url, &archive).await?;
                report(app, tool, "extracting", 0, 0, None);
                extract(archive, bin.to_path_buf()).await?;
            }
            Ok(())
        }
    }
}

async fn download(app: &AppHandle, tool: Tool, url: &str, dest: &Path) -> Result<(), String> {
    download_to(url, dest, |received, total| report(app, tool, "downloading", received, total, None)).await
}

/// Downloads `url` into `dest`, calling `progress(received, total)` a few times a second (total is 0 if unknown).
pub(crate) async fn download_to(url: &str, dest: &Path, mut progress: impl FnMut(u64, u64)) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .user_agent(concat!("ytdlp-ui/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(20))
        .build()
        .map_err(|e| e.to_string())?;
    let response = client
        .get(url)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("Couldn't download {url}: {e}"))?;

    let total = response.content_length().unwrap_or(0);
    let mut file = tokio::fs::File::create(dest).await.map_err(|e| e.to_string())?;
    let mut stream = response.bytes_stream();
    let mut received = 0;
    let mut last_report = Instant::now();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("Download interrupted: {e}"))?;
        file.write_all(&chunk).await.map_err(|e| e.to_string())?;
        received += chunk.len() as u64;
        if last_report.elapsed() > Duration::from_millis(150) {
            progress(received, total);
            last_report = Instant::now();
        }
    }
    file.flush().await.map_err(|e| e.to_string())?;
    progress(received, total);
    Ok(())
}

/// Pulls the programs we need out of a release archive, flattening folders.
async fn extract(archive: PathBuf, bin: PathBuf) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        let name = archive.to_string_lossy();
        if name.ends_with(".zip") {
            extract_zip(&archive, &bin)
        } else {
            extract_tar(&archive, &bin)
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

fn wanted(file_name: &str) -> bool {
    let stem = file_name.strip_suffix(".exe").unwrap_or(file_name);
    matches!(stem, "ffmpeg" | "ffprobe" | "deno") || file_name.ends_with(".dll")
}

fn extract_zip(archive: &Path, bin: &Path) -> Result<(), String> {
    let file = std::fs::File::open(archive).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| e.to_string())?;
        let Some(path) = entry.enclosed_name() else { continue };
        let Some(file_name) = path.file_name().and_then(|n| n.to_str()).map(String::from) else { continue };
        if entry.is_dir() || !wanted(&file_name) {
            continue;
        }
        let dest = bin.join(&file_name);
        let mut out = std::fs::File::create(&dest).map_err(|e| e.to_string())?;
        std::io::copy(&mut entry, &mut out).map_err(|e| e.to_string())?;
        make_executable(&dest)?;
    }
    Ok(())
}

/// Linux ffmpeg builds come as .tar.xz, which every Linux system's `tar` can unpack.
fn extract_tar(archive: &Path, bin: &Path) -> Result<(), String> {
    let unpacked = archive.with_extension("dir");
    std::fs::create_dir_all(&unpacked).map_err(|e| e.to_string())?;
    let status = std::process::Command::new("tar")
        .arg("-xJf")
        .arg(archive)
        .arg("-C")
        .arg(&unpacked)
        .status()
        .map_err(|e| e.to_string())?;
    if !status.success() {
        return Err("Couldn't unpack ffmpeg".into());
    }
    copy_wanted(&unpacked, bin)
}

fn copy_wanted(dir: &Path, bin: &Path) -> Result<(), String> {
    for entry in std::fs::read_dir(dir).map_err(|e| e.to_string())?.flatten() {
        let path = entry.path();
        if path.is_dir() {
            copy_wanted(&path, bin)?;
        } else if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            if wanted(name) {
                let dest = bin.join(name);
                std::fs::copy(&path, &dest).map_err(|e| e.to_string())?;
                make_executable(&dest)?;
            }
        }
    }
    Ok(())
}

pub(crate) fn make_executable(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
    }
    let _ = path;
    Ok(())
}

fn unsupported() -> String {
    format!("{} on {} isn't supported yet", std::env::consts::OS, std::env::consts::ARCH)
}

fn ytdlp_asset() -> Result<&'static str, String> {
    Ok(match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => "yt-dlp.exe",
        ("windows", "aarch64") => "yt-dlp_arm64.exe",
        ("windows", "x86") => "yt-dlp_x86.exe",
        ("macos", _) => "yt-dlp_macos",
        ("linux", "x86_64") => "yt-dlp_linux",
        ("linux", "aarch64") => "yt-dlp_linux_aarch64",
        _ => return Err(unsupported()),
    })
}

fn deno_target() -> Result<&'static str, String> {
    Ok(match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => "x86_64-pc-windows-msvc",
        ("windows", "aarch64") => "aarch64-pc-windows-msvc",
        ("macos", "x86_64") => "x86_64-apple-darwin",
        ("macos", "aarch64") => "aarch64-apple-darwin",
        ("linux", "x86_64") => "x86_64-unknown-linux-gnu",
        ("linux", "aarch64") => "aarch64-unknown-linux-gnu",
        _ => return Err(unsupported()),
    })
}

fn ffmpeg_urls() -> Result<Vec<String>, String> {
    let builds = "https://github.com/yt-dlp/FFmpeg-Builds/releases/download/latest";
    Ok(match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => vec![format!("{builds}/ffmpeg-master-latest-win64-gpl-shared.zip")],
        ("windows", "aarch64") => vec![format!("{builds}/ffmpeg-master-latest-winarm64-gpl.zip")],
        ("linux", "x86_64") => vec![format!("{builds}/ffmpeg-master-latest-linux64-gpl.tar.xz")],
        ("linux", "aarch64") => vec![format!("{builds}/ffmpeg-master-latest-linuxarm64-gpl.tar.xz")],
        ("macos", arch) => {
            let arch = if arch == "aarch64" { "arm64" } else { "amd64" };
            ["ffmpeg", "ffprobe"]
                .map(|name| format!("https://ffmpeg.martin-riedl.de/redirect/latest/macos/{arch}/release/{name}.zip"))
                .to_vec()
        }
        _ => return Err(unsupported()),
    })
}

/// Runs `yt-dlp -U` and returns the version afterwards.
#[tauri::command]
pub async fn ytdlp_update(app: AppHandle) -> Result<String, String> {
    let (path, _) = locate(&app, Tool::Ytdlp).ok_or("yt-dlp isn't installed")?;
    let output = command(&path)
        .args(["-U", "--no-colors"])
        .output()
        .await
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let reason = stderr.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("Update failed");
        return Err(reason.trim_start_matches("ERROR: ").to_string());
    }
    version(Tool::Ytdlp, &path).await.ok_or_else(|| "Couldn't read the yt-dlp version".into())
}
