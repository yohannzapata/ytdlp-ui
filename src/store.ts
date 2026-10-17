import { create } from "zustand";
import {
  api,
  type CompressChoice,
  type DownloadEvent,
  type DownloadOptions,
  type PlanSource,
  type PlaylistInfo,
  type Quality,
  type Settings,
  type StemsProgress,
  type StemsStatus,
  type ToolStatus,
  type VideoInfo,
} from "./api";
import {
  AUDIO_FORMATS,
  DEFAULT_COMPRESS,
  DEFAULT_STEMS,
  STANDARD_HEIGHTS,
  STEM_FORMATS,
  STEM_PARTS,
  formatsFor,
  isAudioQuality,
  isCompress,
  isProcessing,
  isStems,
  qualityChoices,
  startingQuality,
  type StemsChoice,
} from "./format";

/** A job chosen for a file before it has been read, such as "separate it" from a finished row. */
export type Preset = { stems: StemsChoice } | { compress: CompressChoice };

export type Status = "fetching" | "ready" | "queued" | "downloading" | "processing" | "done" | "error" | "canceled";

export interface Download {
  id: string;
  /** A "link" row is downloaded; a "file" row is a file on this computer that gets converted. */
  source: "link" | "file";
  /** The link to download (for a file: its path). */
  url: string;
  /** For a file: where it is. */
  path: string | null;
  /** For a file: whether it contains video. An audio file can only stay audio. */
  hasVideo: boolean;
  /** For a file: whether it has sound (only then can it be separated into parts). */
  hasAudio: boolean;
  /** What to do with it, chosen before the file has been read. */
  preset: Preset | null;
  /** For a file: its size in bytes. */
  fileSize: number | null;
  title: string;
  channel: string | null;
  duration: number | null;
  thumbnail: string | null;
  /** Resolutions this video really has, or null when unknown (playlist entries). */
  qualities: Quality[] | null;
  audioSize: number | null;
  /** "best", a height such as "1080", "audio", or "stems" (separate the sound into the parts below). */
  quality: string;
  /** With the "stems" quality: the parts to save as their own files, and whether to also save the rest. */
  stemParts: string[];
  stemRest: boolean;
  /** With the "compress" quality: how small to make it. */
  compress: CompressChoice;
  /** Frames per second, when the site says. */
  fps: number | null;
  /** For a link being compressed to a size: the resolution it will most likely end up at, so it isn't downloaded bigger. */
  compressHeight: number | null;
  /** For a link that is separated or compressed: the download it works from, once that's done. */
  sourcePath: string | null;
  /** True while that download runs (the first of the two steps). */
  sourceStep: boolean;
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
const STEMS_KEY = "ytdlp-ui.stems.v1";
const COMPRESS_KEY = "ytdlp-ui.compress.v1";

/** How small the last file was made, so the next one starts from that. */
function lastCompressChoice(): CompressChoice {
  try {
    const saved = JSON.parse(localStorage.getItem(COMPRESS_KEY) ?? "null") as CompressChoice | null;
    const goal = saved?.goal;
    if (goal && (goal.mode === "same" || (goal.mode === "size" && goal.bytes > 0))) {
      return { goal, newerFormat: !!saved.newerFormat };
    }
  } catch {
    /* nothing remembered */
  }
  return DEFAULT_COMPRESS;
}

function rememberCompressChoice(choice: CompressChoice) {
  try {
    localStorage.setItem(COMPRESS_KEY, JSON.stringify(choice));
  } catch {
    /* the choice just won't be remembered */
  }
}

/** The audio format a compressed sound file keeps: its own when the app can write it. */
function audioFormatOf(path: string, fallback: string): string {
  const ext = path.split(".").pop()?.toLowerCase() ?? "";
  return AUDIO_FORMATS.includes(ext) ? ext : AUDIO_FORMATS.includes(fallback) ? fallback : AUDIO_FORMATS[0];
}

/** The parts chosen last time, so the next file starts from them. */
function lastStemsChoice(): StemsChoice {
  try {
    const saved = JSON.parse(localStorage.getItem(STEMS_KEY) ?? "null") as StemsChoice | null;
    const valid = STEM_PARTS.map((p) => p.value);
    if (saved && Array.isArray(saved.parts) && saved.parts.length && saved.parts.every((p) => valid.includes(p))) {
      return { parts: saved.parts, rest: !!saved.rest };
    }
  } catch {
    /* nothing remembered */
  }
  return DEFAULT_STEMS;
}

function rememberStemsChoice(choice: StemsChoice) {
  try {
    localStorage.setItem(STEMS_KEY, JSON.stringify(choice));
  } catch {
    /* the choice just won't be remembered */
  }
}

interface AppState {
  settings: Settings | null;
  tools: ToolStatus[] | null;
  downloads: Download[];
  selectedId: string | null;
  playlists: PendingPlaylist[];
  /** The stem-separation add-on: installed or not, and how big. */
  stems: StemsStatus | null;
  /** How the install is going while it runs (or what went wrong). */
  stemsInstall: { phase: string; detail: string; received: number; total: number; error: string | null } | null;
  /** Set while the "install audio separation?" dialog is open; `then` runs once it has been installed. */
  stemsPrompt: { then: (() => void) | null } | null;
  /** Set while the "which parts?" dialog is open: for a row of the list (`id`) or a finished file (`path`). */
  stemsPicker: { id: string | null; path: string | null; initial: StemsChoice } | null;
  /** Set while the "how small?" dialog is open: for a row of the list (`id`) or a finished file (`path`). */
  compressPicker: {
    id: string | null;
    path: string | null;
    hasVideo: boolean;
    format: string;
    source: PlanSource;
    initial: CompressChoice;
  } | null;

  load(): Promise<void>;
  refreshTools(): Promise<void>;
  updateSettings(patch: Partial<Settings>): void;
  updateOptions(patch: Partial<DownloadOptions>): void;

  /** Adds every link found in `text`; returns how many. */
  addLinks(text: string): number;
  addFiles(paths: string[], preset?: Preset): number;
  /** Opens the "which parts?" dialog for a row of the list, or for a finished file (which is then added as a row). */
  chooseStems(target: { id: string } | { path: string }): void;
  confirmStems(choice: StemsChoice): void;
  dismissStemsPicker(): void;
  /** Opens the "how small?" dialog for a row of the list, or for a finished file (which is then added as a row). */
  chooseCompress(target: { id: string } | { path: string; hasVideo: boolean }): void;
  /** `height` is where the plan expects the video to end up, so a link isn't downloaded bigger than that. */
  confirmCompress(choice: CompressChoice, height: number | null): void;
  dismissCompressPicker(): void;
  refreshStems(): Promise<void>;
  requestStems(then?: () => void): void;
  dismissStemsPrompt(): void;
  installStems(gpu: boolean): Promise<void>;
  cancelStemsInstall(): void;
  removeStems(): Promise<void>;
  handleStemsEvent(event: StemsProgress): void;
  addPlaylist(id: string, indexes: number[], quality: string, format: string): void;
  dismissPlaylist(id: string): void;

  select(id: string | null): void;
  setChoice(
    id: string,
    choice: { quality?: string; format?: string; stems?: StemsChoice; compress?: CompressChoice; compressHeight?: number | null },
  ): void;
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

/** The quality choices a row offers. */
export const choicesFor = (d: Pick<Download, "qualities" | "audioSize" | "source" | "hasVideo" | "hasAudio">) =>
  qualityChoices(d.qualities, d.audioSize, d.source === "file" ? { hasVideo: d.hasVideo, hasAudio: d.hasAudio } : undefined);

const blank = (): Omit<Download, "id" | "url" | "title" | "quality" | "format"> => ({
  source: "link",
  path: null,
  hasVideo: true,
  hasAudio: true,
  preset: null,
  stemParts: DEFAULT_STEMS.parts,
  stemRest: DEFAULT_STEMS.rest,
  compress: DEFAULT_COMPRESS,
  compressHeight: null,
  fps: null,
  sourcePath: null,
  sourceStep: false,
  fileSize: null,
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
      // A link that is separated or compressed is downloaded first, as a step of its own.
      const needsSource = d.source === "link" && isProcessing(d.quality) && !d.sourcePath;
      // Separating and compressing use every processor core, so one of each runs at a time; the others wait.
      const busy = get().downloads.some(
        (x) => x.id !== d.id && ACTIVE.includes(x.status) && !x.sourceStep && x.quality === d.quality,
      );
      if (isProcessing(d.quality) && !needsSource && busy) continue;
      active++;
      // The second step of a link keeps the log of the first.
      patch(d.id, { status: "downloading", percent: null, error: null, stage: null, sourceStep: needsSource, ...(d.sourcePath ? {} : { log: [] }) });
      const audioOnly = d.quality === "audio";
      const maxHeight = audioOnly || d.quality === "best" || isProcessing(d.quality) ? null : Number(d.quality);
      const input = d.sourcePath ?? d.path ?? d.url;
      const removeInput = d.sourcePath !== null;
      const run = needsSource
        ? api.startDownload({
            id: d.id,
            url: d.url,
            // Separating only needs the sound; a size goal only needs the resolution it will end up at.
            audioOnly: isStems(d.quality) || !d.hasVideo,
            maxHeight:
              isCompress(d.quality) && d.compress.goal.mode === "size" && d.compressHeight
                ? Math.max(d.compressHeight, 720)
                : null,
            format: "mkv",
            folder: settings.downloadDir,
            subfolder: null,
            options: settings.options,
            forProcessing: true,
          })
        : isStems(d.quality)
        ? api.startSeparate({
            id: d.id,
            path: input,
            parts: d.stemParts,
            rest: d.stemRest,
            format: d.format,
            folder: settings.downloadDir,
            removeInput,
          })
        : isCompress(d.quality)
        ? api.startCompress({
            id: d.id,
            path: input,
            choice: d.compress,
            format: d.format,
            folder: settings.downloadDir,
            options: settings.options,
            removeInput,
          })
        : d.source === "file"
          ? api.startProcess({
              id: d.id,
              path: d.path ?? d.url,
              audioOnly,
              maxHeight,
              format: d.format,
              folder: settings.downloadDir,
              options: settings.options,
            })
          : api.startDownload({
              id: d.id,
              url: d.url,
              audioOnly,
              maxHeight,
              format: d.format,
              folder: settings.downloadDir,
              subfolder: d.subfolder,
              options: settings.options,
            });
      run.catch((err) => {
          // A step that couldn't start has already removed its source download.
          patch(d.id, { status: "error", error: String(err), sourcePath: null, sourceStep: false });
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

  /** Reads what is inside a file on this computer. */
  const lookupFile = async (item: Download) => {
    const { settings } = get();
    if (!settings) return;
    try {
      const info = await api.probeFile(item.path ?? item.url);
      if (!get().downloads.some((d) => d.id === item.id)) return; // removed while looking
      const qualities: Quality[] =
        info.hasVideo && info.height
          ? [info.height, ...STANDARD_HEIGHTS.filter((h) => h < info.height!)].map((height) => ({ height, size: null }))
          : [];
      const choices = qualityChoices(qualities, null, { hasVideo: info.hasVideo, hasAudio: info.hasAudio });
      const preset = item.preset;
      const separate = preset !== null && "stems" in preset && choices.some((c) => c.value === "stems");
      const shrink = preset !== null && "compress" in preset && choices.some((c) => c.value === "compress");
      const quality = separate ? "stems" : shrink ? "compress" : startingQuality(settings.quality, choices);
      const shrinkFormat = info.hasVideo ? "mp4" : audioFormatOf(item.path ?? item.url, settings.audioFormat);
      const stemFormat = STEM_FORMATS.includes(settings.audioFormat) ? settings.audioFormat : STEM_FORMATS[0];
      patch(item.id, {
        hasAudio: info.hasAudio,
        title: info.title,
        channel: info.artist,
        duration: info.duration,
        thumbnail: info.thumbnail,
        qualities,
        audioSize: null,
        hasVideo: info.hasVideo,
        fileSize: info.size,
        quality,
        ...(separate && "stems" in preset ? { stemParts: preset.stems.parts, stemRest: preset.stems.rest } : {}),
        ...(shrink && "compress" in preset ? { compress: preset.compress } : {}),
        format: isStems(quality)
          ? stemFormat
          : isCompress(quality)
            ? shrinkFormat
            : quality === "audio"
              ? settings.audioFormat
              : settings.videoFormat,
        needsInfo: false,
        status: settings.autoStart ? "queued" : "ready",
      });
      pump();
    } catch (err) {
      patch(item.id, { status: "error", error: String(err), needsInfo: true });
    }
  };

  const lookup = async (item: Download) => {
    if (item.source === "file") return lookupFile(item);
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
      hasVideo: info.qualities.length > 0,
      fps: info.fps,
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
    stems: null,
    stemsInstall: null,
    stemsPrompt: null,
    stemsPicker: null,
    compressPicker: null,

    async load() {
      const [settings] = await Promise.all([api.getSettings(), get().refreshTools(), get().refreshStems()]);
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

    addFiles(paths, preset) {
      const settings = get().settings;
      if (!settings || !paths.length) return 0;
      const last = lastStemsChoice();
      const added: Download[] = paths.map((path) => ({
        ...blank(),
        id: crypto.randomUUID(),
        source: "file",
        url: path,
        path,
        title: path.split(/[\\/]/).pop() ?? path,
        preset: preset ?? null,
        stemParts: last.parts,
        stemRest: last.rest,
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

    chooseStems(target) {
      const row = "id" in target ? get().downloads.find((d) => d.id === target.id) : undefined;
      const initial = row?.quality === "stems" ? { parts: row.stemParts, rest: row.stemRest } : lastStemsChoice();
      set({ stemsPicker: { id: "id" in target ? target.id : null, path: "path" in target ? target.path : null, initial } });
    },

    confirmStems(choice) {
      const picker = get().stemsPicker;
      if (!picker) return;
      set({ stemsPicker: null });
      rememberStemsChoice(choice);
      if (picker.id) {
        get().setChoice(picker.id, { quality: "stems", stems: choice });
      } else if (picker.path) {
        const path = picker.path;
        const go = () => get().addFiles([path], { stems: choice });
        if (get().stems?.installed) go();
        else get().requestStems(go);
      }
    },

    dismissStemsPicker() {
      set({ stemsPicker: null });
    },

    chooseCompress(target) {
      const settings = get().settings;
      if (!settings) return;
      if ("id" in target) {
        const d = get().downloads.find((x) => x.id === target.id);
        if (!d) return;
        const best = d.qualities?.[0];
        const source: PlanSource =
          d.source === "file"
            ? { path: d.path ?? d.url }
            : {
                duration: d.duration ?? 0,
                height: best?.height ?? 1080,
                fps: d.fps,
                // A quality's size already counts its sound.
                size: best?.size ?? null,
                video: d.hasVideo,
              };
        set({
          compressPicker: {
            id: d.id,
            path: null,
            hasVideo: d.hasVideo,
            format: d.hasVideo ? "mp4" : audioFormatOf(d.path ?? "", settings.audioFormat),
            source,
            initial: isCompress(d.quality) ? d.compress : lastCompressChoice(),
          },
        });
      } else {
        set({
          compressPicker: {
            id: null,
            path: target.path,
            hasVideo: target.hasVideo,
            format: target.hasVideo ? "mp4" : audioFormatOf(target.path, settings.audioFormat),
            source: { path: target.path },
            initial: lastCompressChoice(),
          },
        });
      }
    },

    confirmCompress(choice, height) {
      const picker = get().compressPicker;
      if (!picker) return;
      set({ compressPicker: null });
      rememberCompressChoice(choice);
      if (picker.id) get().setChoice(picker.id, { quality: "compress", compress: choice, compressHeight: height });
      else if (picker.path) get().addFiles([picker.path], { compress: choice });
    },

    dismissCompressPicker() {
      set({ compressPicker: null });
    },

    async refreshStems() {
      set({ stems: await api.stemsStatus() });
    },

    requestStems(then) {
      set({ stemsPrompt: { then: then ?? null } });
    },

    dismissStemsPrompt() {
      // While an install is running the only way out is Cancel, so it can't be left running unseen.
      const install = get().stemsInstall;
      if (!install || install.error) set({ stemsPrompt: null, stemsInstall: null });
    },

    async installStems(gpu) {
      set({ stemsInstall: { phase: "uv", detail: "Starting…", received: 0, total: 0, error: null } });
      try {
        await api.stemsInstall(gpu);
        await get().refreshStems();
        const then = get().stemsPrompt?.then;
        set({ stemsInstall: null, stemsPrompt: null });
        then?.();
      } catch (err) {
        await get().refreshStems().catch(() => {});
        if (String(err) === "Canceled") set({ stemsInstall: null });
        else set((s) => ({ stemsInstall: { ...(s.stemsInstall ?? { phase: "error", detail: "", received: 0, total: 0 }), error: String(err) } }));
      }
    },

    cancelStemsInstall() {
      api.stemsCancelInstall().catch(console.error);
    },

    async removeStems() {
      await api.stemsRemove();
      await get().refreshStems();
    },

    handleStemsEvent(event) {
      if (event.phase === "done" || event.phase === "error") return;
      set((s) =>
        s.stemsInstall
          ? { stemsInstall: { ...s.stemsInstall, phase: event.phase, detail: event.detail, received: event.received, total: event.total } }
          : {},
      );
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
      // Splitting a song needs the add-on; offer to install it first and apply the choice afterwards.
      if (isStems(quality) && !get().stems?.installed) {
        get().requestStems(() => get().setChoice(id, choice));
        return;
      }
      const kind = (q: string) =>
        isStems(q) ? "stems" : isCompress(q) ? (d.hasVideo ? "compress" : "audio") : q === "audio" ? "audio" : "video";
      let format = choice.format ?? d.format;
      // Switching between video, audio, stems and compressing swaps in the matching kind of format.
      if (choice.quality && kind(quality) !== kind(d.quality)) {
        const audioFormat = settings.audioFormat;
        format =
          kind(quality) === "stems"
            ? STEM_FORMATS.includes(audioFormat)
              ? audioFormat
              : STEM_FORMATS[0]
            : kind(quality) === "compress"
              ? "mp4"
              : kind(quality) === "audio"
                ? audioFormat
                : settings.videoFormat;
      }
      const formats = formatsFor(quality, d.hasVideo);
      if (!formats.includes(format)) format = formats[0];
      // A link downloaded for a job that's no longer wanted doesn't need to be kept.
      const dropSource = d.sourcePath !== null && quality !== d.quality;
      if (dropSource) api.discardSource(d.sourcePath!).catch(console.error);
      patch(id, {
        quality,
        format,
        ...(dropSource ? { sourcePath: null } : {}),
        ...(choice.stems ? { stemParts: choice.stems.parts, stemRest: choice.stems.rest } : {}),
        ...(choice.compress ? { compress: choice.compress, compressHeight: choice.compressHeight ?? null } : {}),
      });
      // New rows start from your last choice, but separating and compressing are never made the default.
      if (!isProcessing(quality)) {
        get().updateSettings({ quality, ...(isAudioQuality(quality) ? { audioFormat: format } : { videoFormat: format }) });
      }
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
      const d = get().downloads.find((x) => x.id === id);
      if (d?.sourcePath && !ACTIVE.includes(d.status)) api.discardSource(d.sourcePath).catch(console.error);
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
      for (const d of get().downloads) {
        if (d.sourcePath && !ACTIVE.includes(d.status)) api.discardSource(d.sourcePath).catch(console.error);
      }
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
        case "finished": {
          const d = get().downloads.find((x) => x.id === event.id);
          const done = { sourcePath: null, sourceStep: false };
          if (event.ok && d?.sourceStep && event.filepath) {
            // That was only the download; separating or compressing it comes next.
            patch(event.id, { status: "queued", sourcePath: event.filepath, sourceStep: false, percent: null, speed: null, eta: null });
          } else if (event.ok) patch(event.id, { status: "done", filepath: event.filepath, percent: 100, ...done });
          else if (event.canceled) patch(event.id, { status: "canceled", ...done });
          else patch(event.id, { status: "error", error: event.error, ...done });
          pump();
          break;
        }
      }
    },
  };
});

// The queue survives closing the app. Anything that was in progress comes back as waiting to start.
function restoreQueue(): Download[] {
  try {
    const saved = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? "[]") as Download[];
    for (const d of saved) {
      if (d.sourcePath) api.discardSource(d.sourcePath).catch(() => {});
    }
    return saved.map((d) => ({
      ...blank(),
      ...d,
      ...legacyStems(d),
      sourcePath: null,
      sourceStep: false,
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

// Earlier builds stored "stems2" (vocals + instrumental) and "stems4" (all four parts) as the quality.
function legacyStems(d: Download): Partial<Download> {
  const quality = d.quality as string;
  const preset = d.preset as unknown;
  // Earlier builds stored the parts themselves, or a quality name, as the preset.
  const fix: Partial<Download> =
    typeof preset === "string"
      ? { preset: null }
      : preset && typeof preset === "object" && "parts" in preset
        ? { preset: { stems: preset as StemsChoice } }
        : {};
  if (quality === "stems2") return { ...fix, quality: "stems", stemParts: ["vocals"], stemRest: true };
  if (quality === "stems4") return { ...fix, quality: "stems", stemParts: ["vocals", "drums", "bass", "other"], stemRest: false };
  return fix;
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
