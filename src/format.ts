// Small helpers for turning numbers and yt-dlp messages into friendly text.
import type { Quality } from "./api";

export function formatBytes(bytes: number | null | undefined): string {
  if (bytes == null || !isFinite(bytes)) return "";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1000 && unit < units.length - 1) {
    value /= 1024;
    unit++;
  }
  return `${value.toFixed(unit >= 2 ? 1 : 0)} ${units[unit]}`;
}

export function formatDuration(seconds: number | null | undefined): string {
  if (seconds == null) return "";
  const s = Math.round(seconds);
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const rest = String(s % 60).padStart(2, "0");
  return h > 0 ? `${h}:${String(m).padStart(2, "0")}:${rest}` : `${m}:${rest}`;
}

export function formatEta(seconds: number | null | undefined): string {
  if (seconds == null || !isFinite(seconds)) return "";
  const s = Math.round(seconds);
  if (s < 60) return `${s} s left`;
  if (s < 3600) return `${Math.round(s / 60)} min left`;
  return `${Math.floor(s / 3600)} h ${Math.round((s % 3600) / 60)} min left`;
}

export const VIDEO_FORMATS = ["mp4", "mkv", "webm"];
export const AUDIO_FORMATS = ["mp3", "m4a", "opus"];
/** Formats for the separated parts. */
export const STEM_FORMATS = ["mp3", "flac", "wav"];

/** The parts a file's sound can be separated into, in the order they are listed. */
export const STEM_PARTS = [
  { value: "vocals", label: "Vocals", hint: "Singing and speech" },
  { value: "drums", label: "Drums", hint: "" },
  { value: "bass", label: "Bass", hint: "" },
  { value: "guitar", label: "Guitar", hint: "Less exact than the first three" },
  { value: "piano", label: "Piano", hint: "The least exact part" },
  { value: "other", label: "Other", hint: "Synths, strings and everything else" },
];

/** Which parts to save, and whether to also save everything that wasn't chosen as one more file. */
export interface StemsChoice {
  parts: string[];
  rest: boolean;
}

export const DEFAULT_STEMS: StemsChoice = { parts: ["vocals"], rest: true };

/** Without guitar or piano the standard model runs, and its "other" still holds both. */
const FOUR_PARTS = ["vocals", "drums", "bass", "other"];

/** The parts that end up in the extra file: everything that wasn't chosen. Empty when nothing is left. */
export function leftoverParts(parts: string[]): string[] {
  const six = parts.includes("guitar") || parts.includes("piano");
  return (six ? STEM_PARTS.map((p) => p.value) : FOUR_PARTS).filter((p) => !parts.includes(p));
}

/** What the extra file is called, the same way the app names it. */
export function restName(parts: string[]): string {
  if (parts.length === 1 && parts[0] === "vocals") return "instrumental";
  if (parts.length === 1 && parts[0] !== "other") return `no ${parts[0]}`;
  return "everything else";
}

/** A short description of a choice for the quality menu: "Vocals + instrumental", "Drums + Bass + rest", "3 parts". */
export function stemsLabel({ parts, rest }: StemsChoice): string {
  const names = STEM_PARTS.filter((p) => parts.includes(p.value)).map((p) => p.label);
  const withRest = rest && leftoverParts(parts).length > 0;
  if (names.length === 1 && parts[0] === "vocals") return withRest ? "Vocals + instrumental" : "Vocals only";
  const base = names.length <= 2 ? names.join(" + ") : `${names.length} parts`;
  if (withRest) return `${base} + rest`;
  return names.length === 1 ? `${base} only` : base;
}

/** "stems" is the quality choice that separates a file's sound; which parts is stored beside it. */
export const isStems = (quality: string) => quality === "stems";
/** Choices that produce audio only: plain audio, or stems. */
export const isAudioQuality = (quality: string) => quality === "audio" || isStems(quality);
export const formatsFor = (quality: string) =>
  isStems(quality) ? STEM_FORMATS : quality === "audio" ? AUDIO_FORMATS : VIDEO_FORMATS;

/** Quality choices shown for playlists, where each video's options aren't known up front. */
export const STANDARD_HEIGHTS = [2160, 1440, 1080, 720, 480, 360];

export function qualityLabel(height: number): string {
  if (height >= 2160) return `${height}p · 4K`;
  return `${height}p`;
}

export interface QualityChoice {
  /** "best", a height such as "1080", "audio", or "stems" (separate the sound). */
  value: string;
  label: string;
  height?: number;
  size?: number | null;
}

/**
 * What a row's quality menu offers: the resolutions the video really has, or common ones when that is unknown.
 * For a file on this computer the top choice is "Original", and an audio file can only stay audio.
 */
export function qualityChoices(
  qualities: Quality[] | null,
  audioSize: number | null,
  file?: { hasVideo: boolean; hasAudio: boolean },
): QualityChoice[] {
  // Separating the sound only makes sense for a file that has sound.
  const stems: QualityChoice[] = file?.hasAudio ? [{ value: "stems", label: "Separate audio…" }] : [];
  if (file && !file.hasVideo) return [{ value: "audio", label: "Audio only", size: null }, ...stems];
  if (!qualities) {
    return [
      { value: "best", label: "Best available" },
      ...STANDARD_HEIGHTS.map((h) => ({ value: String(h), label: `Up to ${qualityLabel(h)}`, height: h })),
      { value: "audio", label: "Audio only" },
    ];
  }
  const [best, ...rest] = qualities;
  const top = file ? "Original" : "Best";
  return [
    {
      value: "best",
      label: best ? `${top} (${qualityLabel(best.height)})` : `${top} available`,
      height: best?.height,
      size: best?.size,
    },
    ...rest.map((q) => ({ value: String(q.height), label: qualityLabel(q.height), height: q.height, size: q.size })),
    { value: "audio", label: "Audio only", size: audioSize },
    ...stems,
  ];
}

/** Starts from the last choice, stepping down to the nearest resolution this video has. */
export function startingQuality(saved: string, choices: QualityChoice[]): string {
  if (choices.some((c) => c.value === saved)) return saved;
  const wanted = Number(saved);
  const fit = wanted ? choices.find((c) => c.height && c.height <= wanted) : undefined;
  return fit ? fit.value : choices.some((c) => c.value === "best") ? "best" : choices[0].value;
}

const STAGES: Record<string, string> = {
  Merger: "Merging video and audio…",
  ExtractAudio: "Converting audio…",
  VideoRemuxer: "Converting video…",
  EmbedThumbnail: "Adding cover art…",
  ReadFile: "Reading the file…",
  SaveStems: "Saving the parts…",
  Metadata: "Adding details…",
  FFmpegMetadata: "Adding details…",
  MoveFiles: "Finishing…",
};

export function stageLabel(name: string | undefined): string {
  return (name && STAGES[name]) || "Processing…";
}

const ERRORS: [RegExp, string][] = [
  [/No such file or directory/i, "The file was moved or deleted."],
  [/This is a folder/i, "That's a folder. Add the files inside it instead."],
  [/not a valid URL|Unsupported URL/i, "This link isn't supported."],
  [/Private video/i, "This video is private."],
  [/confirm your age|age-restricted/i, "This video is age-restricted and can't be downloaded without signing in."],
  [/not a bot/i, "YouTube is asking to confirm you're not a bot. Try again in a little while."],
  [/members-only|Join this channel/i, "This video is for channel members only."],
  [/Video unavailable|This video is unavailable|has been removed/i, "This video is unavailable."],
  [/Requested format is not available/i, "That quality isn't available for this video."],
  [/HTTP Error 403/i, "The download was blocked. Try updating yt-dlp in Settings."],
  [/getaddrinfo|Unable to download webpage|timed out|Connection/i, "Couldn't connect. Check your internet connection."],
  [/download folder doesn't exist/i, "The download folder doesn't exist. Choose another one in Settings."],
];

export function friendlyError(message: string | null | undefined): string {
  if (!message) return "Something went wrong.";
  const match = ERRORS.find(([pattern]) => pattern.test(message));
  return match ? match[1] : message;
}
