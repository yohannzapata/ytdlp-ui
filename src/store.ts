import { create } from "zustand";
import {
  api,
  type DownloadEvent,
  type DownloadOptions,
  type PlaylistInfo,
  type Quality,
  type Settings,
  type ToolStatus,
  type VideoInfo,
} from "./api";
import { AUDIO_FORMATS, VIDEO_FORMATS, qualityChoices, startingQuality } from "./format";

export type Status = "fetching" | "ready" | "queued" | "downloading" | "processing" | "done" | "error" | "canceled";

export interface Download {
  id: string;
  /** The link to download. */
  url: string;
  title: string;
  channel: string | null;
  duration: number | null;
  thumbnail: string | null;
  /** Resolutions this video really has, or null when unknown (playlist entries). */
  qualities: Quality[] | null;
  audioSize: number | null;
  /** "best", a height such as "1080", or "audio". */
  quality: string;
  format: string;
  /** Playlist downloads go into a folder named after the playlist. */
  subfolder: string | null;
  /** True until the video's details have been fetched. */
  needsInfo: boolean;
  status: Status;
  /** Which stream is downloading (video, then audio) and how many there are. */
  part: number;
  parts: number;
  percent: number | null;
  downloaded: number | null;
  total: number | null;
  speed: number | null;
  eta: number | null;
  stage: string | null;
  filepath: string | null;
  error: string | null;
  log: string[];
}

/** A playlist link whose videos the user still has to choose from. */
export interface PendingPlaylist {
  id: string;
  info: PlaylistInfo;
}

const ACTIVE: Status[] = ["downloading", "processing"];
const FINISHED: Status[] = ["done", "error", "canceled"];
const LOG_LINES = 300;
const MAX_INFO_LOOKUPS = 3;
const STORAGE_KEY = "ytdlp-ui.queue.v1";

interface AppState {
  settings: Settings | null;
  tools: ToolStatus[] | null;
  downloads: Download[];
  selectedId: string | null;
  playlists: PendingPlaylist[];

  load(): Promise<void>;
  refreshTools(): Promise<void>;
  updateSettings(patch: Partial<Settings>): void;
  updateOptions(patch: Partial<DownloadOptions>): void;

  /** Adds every link found in `text`; returns how many. */
  addLinks(text: string): number;
  addPlaylist(id: string, indexes: number[], quality: string, format: string): void;
  dismissPlaylist(id: string): void;

  select(id: string | null): void;
  setChoice(id: string, choice: { quality?: string; format?: string }): void;
  start(): void;
  stopAll(): void;
  cancel(id: string): void;
  retry(id: string): void;
  retryFailed(): void;
  remove(id: string): void;
  clearFinished(): void;
  clearAll(): void;
  handleEvent(event: DownloadEvent): void;
}

const blank = (): Omit<Download, "id" | "url" | "title" | "quality" | "format"> => ({
  channel: null,
  duration: null,
  thumbnail: null,
  qualities: null,
  audioSize: null,
  subfolder: null,
  needsInfo: false,
  status: "ready",
  part: 0,
  parts: 1,
  percent: null,
  downloaded: null,
  total: null,
  speed: null,
  eta: null,
  stage: null,
  filepath: null,
  error: null,
  log: [],
});

/** Links in pasted text, one per line or separated by spaces. */
export function findLinks(text: string): string[] {
  const links = text.split(/\s+/).filter((word) => /^https?:\/\/\S+$/i.test(word));
  if (links.length) return links;
  const single = text.trim();
  return single && !/\s/.test(single) ? [single] : [];
}

export const useApp = create<AppState>((set, get) => {
  const patch = (id: string, changes: Partial<Download>) =>
    set((s) => ({ downloads: s.downloads.map((d) => (d.id === id ? { ...d, ...changes } : d)) }));

  /** Starts queued downloads while there's room. */
  const pump = () => {
    const { downloads, settings } = get();
    if (!settings) return;
    let active = downloads.filter((d) => ACTIVE.includes(d.status)).length;
    for (const d of downloads) {
      if (active >= settings.maxConcurrent) break;
      if (d.status !== "queued") continue;
      active++;
      patch(d.id, { status: "downloading", percent: null, error: null, stage: null, log: [] });
      const audioOnly = d.quality === "audio";
      api
        .startDownload({
          id: d.id,
          url: d.url,
          audioOnly,
          maxHeight: audioOnly || d.quality === "best" ? null : Number(d.quality),
          format: d.format,
          folder: settings.downloadDir,
          subfolder: d.subfolder,
          options: settings.options,
        })
        .catch((err) => {
          patch(d.id, { status: "error", error: String(err) });
          pump();
        });
    }
  };

  // Looking up a link's details runs a program each time, so only a few at once.
  const lookups: string[] = [];
  let lookupsRunning = 0;
  const nextLookup = () => {
    while (lookupsRunning < MAX_INFO_LOOKUPS && lookups.length) {
      const id = lookups.shift()!;
      const item = get().downloads.find((d) => d.id === id);
      if (!item || item.status !== "fetching") continue;
      lookupsRunning++;
      lookup(item).finally(() => {
        lookupsRunning--;
        nextLookup();
      });
    }
  };
  const enqueueLookup = (id: string) => {
    lookups.push(id);
    nextLookup();
  };

  const lookup = async (item: Download) => {
    const { settings } = get();
    if (!settings) return;
    try {
      const info = await api.fetchInfo(item.url, settings.options.cookiesBrowser);
      if (!get().downloads.some((d) => d.id === item.id)) return; // removed while looking
      if (info.kind === "playlist") {
        set((s) => ({
          downloads: s.downloads.filter((d) => d.id !== item.id),
          playlists: [...s.playlists, { id: item.id, info }],
        }));
        return;
      }
      applyVideo(item.id, info);
    } catch (err) {
      patch(item.id, { status: "error", error: String(err), needsInfo: true });
    }
  };

  const applyVideo = (id: string, info: VideoInfo) => {
    const { settings } = get();
    if (!settings) return;
    const quality = startingQuality(settings.quality, qualityChoices(info.qualities, info.audioSize));
    const format = quality === "audio" ? settings.audioFormat : settings.videoFormat;
    patch(id, {
      title: info.title,
      channel: info.channel,
      duration: info.duration,
      thumbnail: info.thumbnail,
      qualities: info.qualities,
      audioSize: info.audioSize,
      url: info.url || get().downloads.find((d) => d.id === id)?.url || "",
      quality,
      format,
      needsInfo: false,
      status: settings.autoStart ? "queued" : "ready",
    });
    pump();
  };

  return {
    settings: null,
    tools: null,
    downloads: [],
    selectedId: null,
    playlists: [],

    async load() {
      const [settings] = await Promise.all([api.getSettings(), get().refreshTools()]);
      set({ settings, downloads: restoreQueue() });
    },

    async refreshTools() {
      set({ tools: await api.toolsStatus() });
    },

    updateSettings(changes) {
      const current = get().settings;
      if (!current) return;
      const settings = { ...current, ...changes };
      set({ settings });
      api.saveSettings(settings).catch(console.error);
      if (changes.maxConcurrent) pump();
    },

    updateOptions(changes) {
      const current = get().settings;
      if (current) get().updateSettings({ options: { ...current.options, ...changes } });
    },

    addLinks(text) {
      const settings = get().settings;
      const links = findLinks(text);
      if (!settings || !links.length) return 0;
      const added: Download[] = links.map((url) => ({
        ...blank(),
        id: crypto.randomUUID(),
        url,
        title: url,
        quality: settings.quality === "audio" ? "audio" : "best",
        format: settings.quality === "audio" ? settings.audioFormat : settings.videoFormat,
        needsInfo: true,
        status: "fetching",
      }));
      set((s) => ({ downloads: [...s.downloads, ...added] }));
      added.forEach((d) => enqueueLookup(d.id));
      return added.length;
    },

    addPlaylist(id, indexes, quality, format) {
      const pending = get().playlists.find((p) => p.id === id);
      const settings = get().settings;
      if (!pending || !settings) return;
      const { info } = pending;
      const added: Download[] = indexes.map((i) => {
        const entry = info.entries[i];
        return {
          ...blank(),
          id: crypto.randomUUID(),
          url: entry.url,
          title: entry.title,
          channel: entry.channel ?? info.channel,
          duration: entry.duration,
          thumbnail: entry.thumbnail,
          quality,
          format,
          subfolder: info.title,
          status: settings.autoStart ? "queued" : "ready",
        };
      });
      set((s) => ({
        downloads: [...s.downloads, ...added],
        playlists: s.playlists.filter((p) => p.id !== id),
      }));
      pump();
    },

    dismissPlaylist(id) {
      set((s) => ({ playlists: s.playlists.filter((p) => p.id !== id) }));
    },

    select(id) {
      set({ selectedId: id });
    },

    setChoice(id, choice) {
      const d = get().downloads.find((x) => x.id === id);
      const settings = get().settings;
      if (!d || !settings) return;
      const quality = choice.quality ?? d.quality;
      const audio = quality === "audio";
      let format = choice.format ?? d.format;
      // Switching between video and audio swaps in the matching kind of format.
      if (choice.quality && (quality === "audio") !== (d.quality === "audio")) {
        format = audio ? settings.audioFormat : settings.videoFormat;
      }
      if (!(audio ? AUDIO_FORMATS : VIDEO_FORMATS).includes(format)) format = audio ? AUDIO_FORMATS[0] : VIDEO_FORMATS[0];
      patch(id, { quality, format });
      get().updateSettings({ quality, ...(audio ? { audioFormat: format } : { videoFormat: format }) });
    },

    start() {
      set((s) => ({ downloads: s.downloads.map((d) => (d.status === "ready" ? { ...d, status: "queued" } : d)) }));
      pump();
    },

    stopAll() {
      for (const d of get().downloads) {
        if (d.status === "queued") patch(d.id, { status: "ready" });
        else if (ACTIVE.includes(d.status)) api.cancelDownload(d.id);
      }
    },

    cancel(id) {
      const d = get().downloads.find((x) => x.id === id);
      if (!d) return;
      if (d.status === "queued") patch(id, { status: "ready" });
      else if (d.status === "fetching") patch(id, { status: "canceled", needsInfo: true });
      else if (ACTIVE.includes(d.status)) api.cancelDownload(id);
    },

    retry(id) {
      const d = get().downloads.find((x) => x.id === id);
      if (!d) return;
      patch(id, { error: null, percent: null });
      if (d.needsInfo) {
        patch(id, { status: "fetching" });
        enqueueLookup(id);
      } else {
        patch(id, { status: "queued" });
        pump();
      }
    },

    retryFailed() {
      get()
        .downloads.filter((d) => d.status === "error" || d.status === "canceled")
        .forEach((d) => get().retry(d.id));
    },

    remove(id) {
      get().cancel(id);
      set((s) => ({
        downloads: s.downloads.filter((d) => d.id !== id),
        selectedId: s.selectedId === id ? null : s.selectedId,
      }));
    },

    clearFinished() {
      set((s) => {
        const downloads = s.downloads.filter((d) => !FINISHED.includes(d.status));
        return { downloads, selectedId: downloads.some((d) => d.id === s.selectedId) ? s.selectedId : null };
      });
    },

    clearAll() {
      get().stopAll();
      set({ downloads: [], selectedId: null });
    },

    handleEvent(event) {
      switch (event.type) {
        case "progress": {
          const { part, parts, percent, downloaded, total } = event;
          // yt-dlp reports the speed of the last moment, which jumps around; smooth it so the time left stays steady.
          const before = get().downloads.find((d) => d.id === event.id);
          const previous = before && before.part === part ? before.speed : null;
          const speed = event.speed == null ? previous : previous == null ? event.speed : previous * 0.7 + event.speed * 0.3;
          const eta =
            speed && total != null && downloaded != null ? Math.max(0, (total - downloaded) / speed) : event.eta;
          patch(event.id, { status: "downloading", part, parts, percent, downloaded, total, speed, eta });
          break;
        }
        case "stage":
          patch(event.id, { status: "processing", stage: event.name });
          break;
        case "log":
          set((s) => ({
            downloads: s.downloads.map((d) =>
              d.id === event.id ? { ...d, log: [...d.log.slice(-(LOG_LINES - 1)), event.line] } : d,
            ),
          }));
          break;
        case "finished":
          if (event.ok) patch(event.id, { status: "done", filepath: event.filepath, percent: 100 });
          else if (event.canceled) patch(event.id, { status: "canceled" });
          else patch(event.id, { status: "error", error: event.error });
          pump();
          break;
      }
    },
  };
});

// The queue survives closing the app. Anything that was in progress comes back as waiting to start.
function restoreQueue(): Download[] {
  try {
    const saved = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? "[]") as Download[];
    return saved.map((d) => ({
      ...blank(),
      ...d,
      log: [],
      status: (["fetching", "queued", "downloading", "processing"] as Status[]).includes(d.status) ? "ready" : d.status,
      percent: d.status === "done" ? 100 : null,
      speed: null,
      eta: null,
    }));
  } catch {
    return [];
  }
}

let saveTimer: ReturnType<typeof setTimeout> | undefined;
useApp.subscribe((state, previous) => {
  if (state.downloads === previous.downloads || !state.settings) return;
  clearTimeout(saveTimer);
  saveTimer = setTimeout(() => {
    try {
      const slim = state.downloads.map(({ log: _log, ...rest }) => rest);
      localStorage.setItem(STORAGE_KEY, JSON.stringify(slim));
    } catch {
      /* storage unavailable: the queue just won't be remembered */
    }
  }, 500);
});

export const isActive = (d: Download) => ACTIVE.includes(d.status);
export const isFinished = (d: Download) => FINISHED.includes(d.status);
