import { useEffect, useMemo, useState } from "react";
import {
  Button,
  Caption1,
  Checkbox,
  Dialog,
  DialogActions,
  DialogBody,
  DialogContent,
  DialogSurface,
  DialogTitle,
  Dropdown,
  Field,
  Input,
  makeStyles,
  MessageBar,
  MessageBarBody,
  MessageBarTitle,
  Option,
  shorthands,
  Spinner,
  Text,
  tokens,
} from "@fluentui/react-components";
import { ArrowDownloadRegular, FolderRegular } from "@fluentui/react-icons";
import { api, chooseFolder, type MediaInfo } from "../api";
import {
  AUDIO_FORMATS,
  formatBytes,
  formatDuration,
  friendlyError,
  qualityLabel,
  STANDARD_HEIGHTS,
  VIDEO_FORMATS,
} from "../format";
import { useApp } from "../store";
import { Thumbnail } from "./Thumbnail";

const useStyles = makeStyles({
  surface: { maxWidth: "600px" },
  confirm: { whiteSpace: "nowrap" },
  content: {
    display: "flex",
    flexDirection: "column",
    gap: "16px",
    paddingTop: "4px",
  },
  loading: { padding: "40px 0" },
  preview: {
    display: "flex",
    gap: "16px",
    alignItems: "flex-start",
  },
  previewText: {
    display: "flex",
    flexDirection: "column",
    gap: "4px",
    minWidth: 0,
  },
  twoLines: {
    display: "-webkit-box",
    WebkitLineClamp: 2,
    WebkitBoxOrient: "vertical",
    overflow: "hidden",
  },
  meta: { color: tokens.colorNeutralForeground3 },
  choices: {
    display: "grid",
    gridTemplateColumns: "1fr 1fr",
    gap: "12px",
  },
  dropdown: { minWidth: "0" },
  optionRow: {
    display: "flex",
    justifyContent: "space-between",
    gap: "16px",
    width: "100%",
  },
  size: { color: tokens.colorNeutralForeground3 },
  folderRow: {
    display: "flex",
    gap: "8px",
  },
  folderInput: { flex: 1, minWidth: 0 },
  listBox: {
    borderRadius: tokens.borderRadiusMedium,
    ...shorthands.border("1px", "solid", tokens.colorNeutralStroke2),
    overflow: "hidden",
  },
  listHeader: {
    padding: "2px 4px",
    backgroundColor: tokens.colorNeutralBackground2,
    ...shorthands.borderBottom("1px", "solid", tokens.colorNeutralStroke2),
  },
  entries: {
    maxHeight: "240px",
    overflowY: "auto",
    padding: "2px 4px",
  },
  entry: { display: "flex", width: "100%", maxWidth: "none" },
  entryLabel: {
    display: "flex",
    alignItems: "center",
    gap: "10px",
    flex: 1,
    minWidth: 0,
  },
  entryIndex: {
    color: tokens.colorNeutralForeground3,
    minWidth: "22px",
    textAlign: "right",
  },
  entryTitle: { minWidth: 0 },
  entryDuration: {
    marginLeft: "auto",
    paddingLeft: "12px",
    flexShrink: 0,
    color: tokens.colorNeutralForeground3,
  },
});

interface QualityOption {
  value: string;
  label: string;
  height?: number;
  size?: number | null;
}

/** Highest quality first, then audio only. */
function qualityOptions(info: MediaInfo): QualityOption[] {
  if (info.kind === "video") {
    const [best, ...rest] = info.qualities;
    return [
      {
        value: "best",
        label: best ? `Best quality (${qualityLabel(best.height)})` : "Best quality",
        height: best?.height,
        size: best?.size,
      },
      ...rest.map((q) => ({ value: String(q.height), label: qualityLabel(q.height), height: q.height, size: q.size })),
      { value: "audio", label: "Audio only", size: info.audioSize },
    ];
  }
  return [
    { value: "best", label: "Best quality" },
    ...STANDARD_HEIGHTS.map((h) => ({ value: String(h), label: `Up to ${qualityLabel(h)}`, height: h })),
    { value: "audio", label: "Audio only" },
  ];
}

/** Starts from the last choice, stepping down to the nearest resolution this video has. */
function initialQuality(saved: string, options: QualityOption[]): string {
  if (options.some((o) => o.value === saved)) return saved;
  const wanted = Number(saved);
  const fit = wanted ? options.find((o) => o.height && o.height <= wanted) : undefined;
  return fit ? fit.value : "best";
}

interface Props {
  url: string | null;
  onClose(): void;
  onAdded(): void;
}

export function AddDialog({ url, onClose, onAdded }: Props) {
  const styles = useStyles();
  const settings = useApp((s) => s.settings)!;
  const { add, updateSettings } = useApp.getState();

  const [info, setInfo] = useState<MediaInfo | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [quality, setQuality] = useState("best");
  const [videoFormat, setVideoFormat] = useState(settings.videoFormat);
  const [audioFormat, setAudioFormat] = useState(settings.audioFormat);
  const [selected, setSelected] = useState<Set<number>>(new Set());

  useEffect(() => {
    if (url === null) return;
    let stale = false;
    setInfo(null);
    setError(null);
    api
      .fetchInfo(url)
      .then((result) => {
        if (stale) return;
        setInfo(result);
        setQuality(initialQuality(settings.quality, qualityOptions(result)));
        setVideoFormat(settings.videoFormat);
        setAudioFormat(settings.audioFormat);
        if (result.kind === "playlist") setSelected(new Set(result.entries.map((_, i) => i)));
      })
      .catch((err) => !stale && setError(String(err)));
    return () => {
      stale = true;
    };
    // Settings are only the starting point for each new link, so they aren't dependencies.
  }, [url]);

  const options = useMemo(() => (info ? qualityOptions(info) : []), [info]);
  const audioOnly = quality === "audio";
  const formats = audioOnly ? AUDIO_FORMATS : VIDEO_FORMATS;
  const format = audioOnly ? audioFormat : videoFormat;
  const setFormat = audioOnly ? setAudioFormat : setVideoFormat;
  const count = info?.kind === "playlist" ? selected.size : 1;

  const confirm = () => {
    if (!info) return;
    const common = {
      audioOnly,
      maxHeight: audioOnly || quality === "best" ? null : Number(quality),
      format,
      folder: settings.downloadDir,
    };
    if (info.kind === "video") {
      add([{ ...common, url: info.url || url!, title: info.title, channel: info.channel, thumbnail: info.thumbnail, subfolder: null }]);
    } else {
      add(
        info.entries
          .filter((_, i) => selected.has(i))
          .map((e) => ({
            ...common,
            url: e.url,
            title: e.title,
            channel: e.channel ?? info.channel,
            thumbnail: e.thumbnail,
            subfolder: info.title,
          })),
      );
    }
    updateSettings({ quality, videoFormat, audioFormat });
    onAdded();
  };

  const toggle = (i: number) =>
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(i)) next.delete(i);
      else next.add(i);
      return next;
    });

  const changeFolder = async () => {
    const folder = await chooseFolder(settings.downloadDir);
    if (folder) updateSettings({ downloadDir: folder });
  };

  const renderBody = () => {
    if (error) {
      return (
        <MessageBar intent="error" layout="multiline">
          <MessageBarBody>
            <MessageBarTitle>Couldn't get this link</MessageBarTitle>
            {friendlyError(error)}
          </MessageBarBody>
        </MessageBar>
      );
    }
    if (!info) return <Spinner className={styles.loading} label="Getting video info…" />;

    const entries = info.kind === "playlist" ? info.entries : [];
    const allSelected = selected.size === entries.length;
    const meta =
      info.kind === "video"
        ? [info.channel, formatDuration(info.duration)]
        : [info.channel, `${entries.length} ${entries.length === 1 ? "video" : "videos"}`];

    return (
      <>
        <div className={styles.preview}>
          <Thumbnail src={info.kind === "video" ? info.thumbnail : (entries[0]?.thumbnail ?? null)} width={160} />
          <div className={styles.previewText}>
            <Text weight="semibold" className={styles.twoLines}>
              {info.title}
            </Text>
            <Caption1 className={styles.meta}>{meta.filter(Boolean).join(" · ")}</Caption1>
          </div>
        </div>

        {info.kind === "playlist" && (
          <div className={styles.listBox}>
            <div className={styles.listHeader}>
              <Checkbox
                checked={allSelected ? true : selected.size === 0 ? false : "mixed"}
                onChange={() => setSelected(allSelected ? new Set() : new Set(entries.map((_, i) => i)))}
                label={`Select all (${selected.size} of ${entries.length})`}
              />
            </div>
            <div className={styles.entries}>
              {entries.map((e, i) => (
                <Checkbox
                  key={i}
                  className={styles.entry}
                  checked={selected.has(i)}
                  onChange={() => toggle(i)}
                  label={{
                    className: styles.entryLabel,
                    children: (
                      <>
                        <Caption1 className={styles.entryIndex}>{i + 1}</Caption1>
                        <Text className={styles.entryTitle} truncate wrap={false} title={e.title}>
                          {e.title}
                        </Text>
                        <Caption1 className={styles.entryDuration}>{formatDuration(e.duration)}</Caption1>
                      </>
                    ),
                  }}
                />
              ))}
            </div>
          </div>
        )}

        <div className={styles.choices}>
          <Field label="Quality">
            <Dropdown
              className={styles.dropdown}
              value={options.find((o) => o.value === quality)?.label ?? ""}
              selectedOptions={[quality]}
              onOptionSelect={(_, data) => data.optionValue && setQuality(data.optionValue)}
            >
              {options.map((o) => (
                <Option key={o.value} value={o.value} text={o.label}>
                  <span className={styles.optionRow}>
                    <span>{o.label}</span>
                    {o.size ? <span className={styles.size}>~{formatBytes(o.size)}</span> : null}
                  </span>
                </Option>
              ))}
            </Dropdown>
          </Field>
          <Field label="Format">
            <Dropdown
              className={styles.dropdown}
              value={format.toUpperCase()}
              selectedOptions={[format]}
              onOptionSelect={(_, data) => data.optionValue && setFormat(data.optionValue)}
            >
              {formats.map((f, i) => (
                <Option key={f} value={f} text={f.toUpperCase()}>
                  <span className={styles.optionRow}>
                    <span>{f.toUpperCase()}</span>
                    {i === 0 && <span className={styles.size}>Plays everywhere</span>}
                  </span>
                </Option>
              ))}
            </Dropdown>
          </Field>
        </div>

        <Field label="Save to">
          <div className={styles.folderRow}>
            <Input
              className={styles.folderInput}
              readOnly
              value={settings.downloadDir}
              contentBefore={<FolderRegular />}
              title={settings.downloadDir}
            />
            <Button onClick={changeFolder}>Change…</Button>
          </div>
        </Field>
      </>
    );
  };

  const title = info?.kind === "playlist" ? "Download playlist" : "Download";
  const confirmLabel = info?.kind === "playlist" ? `Download ${count} ${count === 1 ? "video" : "videos"}` : "Download";

  return (
    <Dialog open={url !== null} onOpenChange={(_, data) => !data.open && onClose()}>
      <DialogSurface className={styles.surface}>
        <DialogBody>
          <DialogTitle>{title}</DialogTitle>
          <DialogContent className={styles.content}>{renderBody()}</DialogContent>
          <DialogActions>
            <Button appearance="secondary" onClick={onClose}>
              {error ? "Close" : "Cancel"}
            </Button>
            {info && (
              <Button
                className={styles.confirm}
                appearance="primary"
                icon={<ArrowDownloadRegular />}
                disabled={count === 0}
                onClick={confirm}
              >
                {confirmLabel}
              </Button>
            )}
          </DialogActions>
        </DialogBody>
      </DialogSurface>
    </Dialog>
  );
}
