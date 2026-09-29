import { create } from "zustand";
import { api, type DownloadEvent, type DownloadRequest, type Settings, type ToolStatus } from "./api";

export type Status = "queued" | "downloading" | "processing" | "done" | "error" | "canceled";

export interface Download extends DownloadRequest {
  title: string;
  channel: string | null;
  thumbnail: string | null;
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

export type NewDownload = Omit<DownloadRequest, "id"> & Pick<Download, "title" | "channel" | "thumbnail">;

const ACTIVE: Status[] = ["downloading", "processing"];
const FINISHED: Status[] = ["done", "error", "canceled"];
const LOG_LINES = 200;

interface AppState {
  settings: Settings | null;
  tools: ToolStatus[] | null;
  downloads: Download[];

  load(): Promise<void>;
  refreshTools(): Promise<void>;
  updateSettings(patch: Partial<Settings>): void;
  add(items: NewDownload[]): void;
  cancel(id: string): void;
  retry(id: string): void;
  remove(id: string): void;
  clearFinished(): void;
  handleEvent(event: DownloadEvent): void;
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
      const { id, url, audioOnly, maxHeight, format, folder, subfolder } = d;
      api.startDownload({ id, url, audioOnly, maxHeight, format, folder, subfolder }).catch((err) => {
        patch(id, { status: "error", error: String(err) });
        pump();
      });
    }
  };

  return {
    settings: null,
    tools: null,
    downloads: [],

    async load() {
      const [settings] = await Promise.all([api.getSettings(), get().refreshTools()]);
      set({ settings });
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

    add(items) {
      const added: Download[] = items.map((item) => ({
        ...item,
        id: crypto.randomUUID(),
        status: "queued",
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
      }));
      set((s) => ({ downloads: [...s.downloads, ...added] }));
      pump();
    },

    cancel(id) {
      const d = get().downloads.find((x) => x.id === id);
      if (!d) return;
      if (d.status === "queued") patch(id, { status: "canceled" });
      else if (ACTIVE.includes(d.status)) api.cancelDownload(id);
    },

    retry(id) {
      patch(id, { status: "queued", error: null, percent: null });
      pump();
    },

    remove(id) {
      get().cancel(id);
      set((s) => ({ downloads: s.downloads.filter((d) => d.id !== id) }));
    },

    clearFinished() {
      set((s) => ({ downloads: s.downloads.filter((d) => !FINISHED.includes(d.status)) }));
    },

    handleEvent(event) {
      switch (event.type) {
        case "progress": {
          const { part, parts, percent, downloaded, total, speed, eta } = event;
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

export const isActive = (d: Download) => ACTIVE.includes(d.status);
export const isFinished = (d: Download) => FINISHED.includes(d.status);
