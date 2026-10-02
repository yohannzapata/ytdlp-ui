# Changelog

All notable changes to ytdlp-ui are listed here, newest first.

## 0.3.0 - 2026-10-03

### Added
- **Local files in the queue.** Drop video or audio files on the window, or press **Add files**, and convert them with
  the same Quality and Format menus as downloads: extract audio, change the format, or shrink a video.
- Streams that are already in the right form are copied instead of re-encoded, so extracting AAC audio or moving
  H.264 from MKV to MP4 is instant and lossless.
- Songs keep their cover art, title and artist when converted (except to Opus).
- **Even out volume** option (EBU R128 loudness) for local files.
- Clear messages for folders and for files that are not video or audio.

### Changed
- The queue summary says "in progress", because it now covers conversions as well as downloads.
- The empty-state text mentions dropping files.

## 0.2.0 - 2026-09-29

### Added
- Redesigned main screen: a queue table with a Quality and Format menu on every row, size estimates, live progress,
  and Start / Stop for the whole queue.
- Options panel: download folder, speed limit, subtitles (embed or save an `.srt`), chapters (embed, split or ignore),
  embedded thumbnail and info, SponsorBlock, sign in with browser cookies, and custom arguments.
- Output tab showing what yt-dlp printed for each download.
- Paste several links at once, or press Ctrl+V anywhere in the window.
- The queue is remembered when the app is closed.
- Optional "Start downloads automatically" in Settings.

### Changed
- Steadier speed and time-left readout.
- Subtitle files and split chapters are saved next to the video and share its number when a name is taken.

### Fixed
- The empty queue no longer shows a scrollbar; scrollbars are thinner.

## 0.1.0 - 2026-09-29

### Added
- First release: download videos and playlists from YouTube and 1000+ other sites with yt-dlp.
- Quality and format picker (MP4, MKV, WebM, MP3, M4A, Opus), playlist picker, and a download queue with progress,
  cancel and retry.
- Downloads yt-dlp, FFmpeg and Deno on first launch and keeps yt-dlp up to date.
- Never overwrites an existing file; canceling leaves no partial files.
- Light and dark themes.
