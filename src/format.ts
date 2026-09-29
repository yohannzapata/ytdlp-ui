// Small helpers for turning numbers and yt-dlp messages into friendly text.

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

/** Quality choices shown for playlists, where each video's options aren't known up front. */
export const STANDARD_HEIGHTS = [2160, 1440, 1080, 720, 480, 360];

export function qualityLabel(height: number): string {
  if (height >= 2160) return `${height}p · 4K`;
  return `${height}p`;
}

/** What the queue shows for a download's choices, e.g. "1080p · MP4" or "Audio · MP3". */
export function describeChoice(audioOnly: boolean, maxHeight: number | null, format: string): string {
  const quality = audioOnly ? "Audio" : maxHeight ? `${maxHeight}p` : "Best quality";
  return `${quality} · ${format.toUpperCase()}`;
}

const STAGES: Record<string, string> = {
  Merger: "Merging video and audio…",
  ExtractAudio: "Converting audio…",
  VideoRemuxer: "Converting video…",
  EmbedThumbnail: "Adding cover art…",
  Metadata: "Adding details…",
  FFmpegMetadata: "Adding details…",
  MoveFiles: "Finishing…",
};

export function stageLabel(name: string | undefined): string {
  return (name && STAGES[name]) || "Processing…";
}

const ERRORS: [RegExp, string][] = [
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
