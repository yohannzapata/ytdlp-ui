// Typed wrappers around the Rust commands and events in src-tauri.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";

export type Theme = "system" | "light" | "dark";

export interface DownloadOptions {
  rateLimitValue: string;
  rateLimitUnit: "K" | "M";
  setFileTimeNow: boolean;
  subtitles: "off" | "embed" | "file";
  subtitleLangs: string;
  chapters: "embed" | "split" | "ignore";
  forceKeyframes: boolean;
  embedThumbnail: boolean;
  embedMetadata: boolean;
  sponsorblock: boolean;
  cookiesBrowser: string;
  customArgsEnabled: boolean;
  customArgs: string;
}

export interface Settings {
  downloadDir: string;
  maxConcurrent: number;
  theme: Theme;
  autoUpdate: boolean;
  lastUpdateCheck: number;
  autoStart: boolean;
  quality: string;
  videoFormat: string;
  audioFormat: string;
  panelOpen: boolean;
  panelTab: "options" | "output";
  options: DownloadOptions;
}

export type Tool = "ytdlp" | "ffmpeg" | "deno";

export interface ToolStatus {
  tool: Tool;
  found: boolean;
  source: "app" | "system" | null;
  version: string | null;
}

export interface InstallProgress {
  tool: Tool;
  phase: "downloading" | "extracting" | "done" | "error";
  received: number;
  total: number;
  error: string | null;
}

export interface Quality {
  height: number;
  size: number | null;
}

export interface VideoInfo {
  kind: "video";
  url: string;
  title: string;
  channel: string | null;
  duration: number | null;
  thumbnail: string | null;
  qualities: Quality[];
  audioSize: number | null;
}

export interface PlaylistEntry {
  url: string;
  title: string;
  channel: string | null;
  duration: number | null;
  thumbnail: string | null;
}

export interface PlaylistInfo {
  kind: "playlist";
  title: string;
  channel: string | null;
  entries: PlaylistEntry[];
}

export type MediaInfo = VideoInfo | PlaylistInfo;

export interface DownloadRequest {
  id: string;
  url: string;
  audioOnly: boolean;
  maxHeight: number | null;
  format: string;
  folder: string;
  subfolder: string | null;
  options: DownloadOptions;
}

export type DownloadEvent =
  | {
      type: "progress";
      id: string;
      part: number;
      parts: number;
      percent: number | null;
      downloaded: number | null;
      total: number | null;
      speed: number | null;
      eta: number | null;
    }
  | { type: "stage"; id: string; name: string }
  | { type: "log"; id: string; line: string }
  | { type: "finished"; id: string; ok: boolean; canceled: boolean; filepath: string | null; error: string | null };

export const api = {
  getSettings: () => invoke<Settings>("get_settings"),
  saveSettings: (settings: Settings) => invoke<void>("save_settings", { settings }),
  toolsStatus: () => invoke<ToolStatus[]>("tools_status"),
  toolsInstall: () => invoke<void>("tools_install"),
  updateYtdlp: () => invoke<string>("ytdlp_update"),
  fetchInfo: (url: string, cookiesBrowser: string) => invoke<MediaInfo>("fetch_info", { url, cookiesBrowser }),
  startDownload: (req: DownloadRequest) => invoke<void>("start_download", { req }),
  cancelDownload: (id: string) => invoke<void>("cancel_download", { id }),
  openFile: (path: string) => invoke<void>("open_file", { path }),
  showInFolder: (path: string) => invoke<void>("show_in_folder", { path }),
};

/** Asks for a folder; resolves to null if the picker is dismissed. */
export async function chooseFolder(current: string): Promise<string | null> {
  const picked = await open({ directory: true, defaultPath: current || undefined });
  return typeof picked === "string" ? picked : null;
}

export const onDownloadEvent =(handler: (event: DownloadEvent) => void) =>
  listen<DownloadEvent>("download", (e) => handler(e.payload));

export const onInstallProgress = (handler: (event: InstallProgress) => void) =>
  listen<InstallProgress>("tools-progress", (e) => handler(e.payload));
