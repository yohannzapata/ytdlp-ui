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
  normalizeAudio: boolean;
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
  /** Frames per second of the video. */
  fps: number | null;
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

/** What was found in a file on this computer. */
export interface MediaFile {
  title: string;
  artist: string | null;
  duration: number | null;
  size: number;
  hasVideo: boolean;
  /** Shorter side of the video in pixels. */
  height: number | null;
  hasAudio: boolean;
  thumbnail: string | null;
}

export interface StemsStatus {
  installed: boolean;
  /** The installed build runs on an NVIDIA graphics card. */
  gpu: boolean;
  /** An NVIDIA graphics card is present, so the faster build can be offered. */
  nvidiaFound: boolean;
  sizeBytes: number;
}

export interface StemsProgress {
  phase: "uv" | "python" | "packages" | "model" | "done" | "error";
  detail: string;
  received: number;
  total: number;
  error: string | null;
}

export interface SeparateRequest {
  id: string;
  path: string;
  /** The parts to save as files of their own. */
  parts: string[];
  /** Also save everything that wasn't chosen, mixed together, as one more file. */
  rest: boolean;
  format: string;
  folder: string;
  /** The file was downloaded only for this; it goes once the job is over. */
  removeInput?: boolean;
}

/** How small to make a file: as small as it gets while it looks the same, or no bigger than a size. */
export type CompressGoal = { mode: "same" } | { mode: "size"; bytes: number; /** chosen as "half the size" */ half?: boolean };

export interface CompressChoice {
  goal: CompressGoal;
  /** The newer video format: smaller again, but slower to make, and not every device plays it. */
  newerFormat: boolean;
}

/** What compressing would do, worked out before it runs. */
export interface CompressPlan {
  /** Shorter side of the video that comes out. */
  height: number | null;
  /** The frame rate, when it comes down. */
  fps: number | null;
  videoKbps: number | null;
  /** About how big the result will be, when that can be known beforehand. */
  estimate: number | null;
  /** Why it can't be done. */
  problem: string | null;
  /** "Same look" won't save much: the file is already stored compactly. */
  alreadySmall: boolean;
  /** The file as it is now (0 when unknown). */
  size: number;
  duration: number | null;
}

/** A file, or what a site says about a link that hasn't been downloaded yet. */
export type PlanSource =
  | { path: string }
  | { duration: number; height: number; fps: number | null; size: number | null; video: boolean };

export interface CompressRequest {
  id: string;
  path: string;
  choice: CompressChoice;
  format: string;
  folder: string;
  options: DownloadOptions;
  removeInput?: boolean;
}

export interface DownloadRequest {
  id: string;
  url: string;
  audioOnly: boolean;
  maxHeight: number | null;
  format: string;
  folder: string;
  subfolder: string | null;
  options: DownloadOptions;
  /** Only the first step of separating or compressing: saved out of sight, as the site has it. */
  forProcessing?: boolean;
}

export interface ProcessRequest {
  id: string;
  path: string;
  audioOnly: boolean;
  maxHeight: number | null;
  format: string;
  folder: string;
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
  probeFile: (path: string) => invoke<MediaFile>("probe_file", { path }),
  startProcess: (req: ProcessRequest) => invoke<void>("start_process", { req }),
  startSeparate: (req: SeparateRequest) => invoke<void>("start_separate", { req }),
  compressPlan: (source: PlanSource, choice: CompressChoice, format: string) =>
    invoke<CompressPlan>("compress_plan", { source, choice: wireChoice(choice), format }),
  startCompress: (req: CompressRequest) => invoke<void>("start_compress", { req: { ...req, choice: wireChoice(req.choice) } }),
  discardSource: (path: string) => invoke<void>("discard_source", { path }),
  stemsStatus: () => invoke<StemsStatus>("stems_status"),
  stemsInstall: (gpu: boolean) => invoke<void>("stems_install", { gpu }),
  stemsCancelInstall: () => invoke<void>("stems_cancel_install"),
  stemsRemove: () => invoke<void>("stems_remove"),
  openFile: (path: string) => invoke<void>("open_file", { path }),
  showInFolder: (path: string) => invoke<void>("show_in_folder", { path }),
};

/** "Half the size" is only how the size was chosen; the app works from the bytes. */
function wireChoice(choice: CompressChoice): CompressChoice {
  const goal = choice.goal.mode === "size" ? { mode: "size" as const, bytes: choice.goal.bytes } : choice.goal;
  return { goal, newerFormat: choice.newerFormat };
}

/** Asks for a folder; resolves to null if the picker is dismissed. */
export async function chooseFolder(current: string): Promise<string | null> {
  const picked = await open({ directory: true, defaultPath: current || undefined });
  return typeof picked === "string" ? picked : null;
}

const MEDIA_EXTENSIONS = [
  "mp4", "mkv", "webm", "mov", "avi", "m4v", "flv", "wmv", "ts", "mpg", "mpeg", "3gp",
  "mp3", "m4a", "aac", "wav", "flac", "ogg", "oga", "opus", "wma", "aiff", "aif",
];

/** Asks for video or audio files; resolves to an empty list if the picker is dismissed. */
export async function chooseFiles(): Promise<string[]> {
  const picked = await open({
    multiple: true,
    filters: [
      { name: "Video and audio", extensions: MEDIA_EXTENSIONS },
      { name: "All files", extensions: ["*"] },
    ],
  });
  return Array.isArray(picked) ? picked : typeof picked === "string" ? [picked] : [];
}

export const onDownloadEvent = (handler: (event: DownloadEvent) => void) =>
  listen<DownloadEvent>("download", (e) => handler(e.payload));

export const onInstallProgress = (handler: (event: InstallProgress) => void) =>
  listen<InstallProgress>("tools-progress", (e) => handler(e.payload));

export const onStemsProgress = (handler: (event: StemsProgress) => void) =>
  listen<StemsProgress>("stems-progress", (e) => handler(e.payload));
