import { useEffect, useState } from "react";
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
  makeStyles,
  Option,
  shorthands,
  Text,
  tokens,
} from "@fluentui/react-components";
import { AddRegular } from "@fluentui/react-icons";
import { AUDIO_FORMATS, formatDuration, qualityChoices, startingQuality, VIDEO_FORMATS } from "../format";
import { useApp } from "../store";
import { Thumbnail } from "./Thumbnail";

const useStyles = makeStyles({
  surface: { maxWidth: "600px" },
  content: { display: "flex", flexDirection: "column", gap: "16px", paddingTop: "4px" },
  preview: { display: "flex", gap: "16px", alignItems: "flex-start" },
  previewText: { display: "flex", flexDirection: "column", gap: "4px", minWidth: 0 },
  twoLines: { display: "-webkit-box", WebkitLineClamp: 2, WebkitBoxOrient: "vertical", overflow: "hidden" },
  meta: { color: tokens.colorNeutralForeground3 },
  choices: { display: "grid", gridTemplateColumns: "1fr 1fr", gap: "12px" },
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
  entries: { maxHeight: "260px", overflowY: "auto", padding: "2px 4px" },
  entry: { display: "flex", width: "100%", maxWidth: "none" },
  entryLabel: { display: "flex", alignItems: "center", gap: "10px", flex: 1, minWidth: 0 },
  entryIndex: { color: tokens.colorNeutralForeground3, minWidth: "22px", textAlign: "right" },
  entryTitle: { minWidth: 0 },
  entryDuration: { marginLeft: "auto", paddingLeft: "12px", flexShrink: 0, color: tokens.colorNeutralForeground3 },
  confirm: { whiteSpace: "nowrap" },
});

/** Shown when a pasted link turns out to be a playlist, so the user can choose which videos to add. */
export function PlaylistDialog() {
  const styles = useStyles();
  const pending = useApp((s) => s.playlists[0]);
  const settings = useApp((s) => s.settings)!;
  const { addPlaylist, dismissPlaylist, updateSettings } = useApp.getState();

  const [selected, setSelected] = useState<Set<number>>(new Set());
  const [quality, setQuality] = useState("best");
  const [videoFormat, setVideoFormat] = useState(settings.videoFormat);
  const [audioFormat, setAudioFormat] = useState(settings.audioFormat);

  const choices = qualityChoices(null, null);
  const id = pending?.id;
  useEffect(() => {
    if (!pending) return;
    setSelected(new Set(pending.info.entries.map((_, i) => i)));
    setQuality(startingQuality(settings.quality, choices));
    setVideoFormat(settings.videoFormat);
    setAudioFormat(settings.audioFormat);
    // Starts from the saved choices each time a new playlist arrives.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [id]);

  const audio = quality === "audio";
  const formats = audio ? AUDIO_FORMATS : VIDEO_FORMATS;
  const format = audio ? audioFormat : videoFormat;
  const setFormat = audio ? setAudioFormat : setVideoFormat;
  const entries = pending?.info.entries ?? [];
  const allSelected = entries.length > 0 && selected.size === entries.length;

  const toggle = (i: number) =>
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(i)) next.delete(i);
      else next.add(i);
      return next;
    });

  const confirm = () => {
    if (!pending) return;
    addPlaylist(pending.id, [...selected].sort((a, b) => a - b), quality, format);
    updateSettings({ quality, videoFormat, audioFormat });
  };

  return (
    <Dialog open={!!pending} onOpenChange={(_, data) => !data.open && pending && dismissPlaylist(pending.id)}>
      <DialogSurface className={styles.surface}>
        <DialogBody>
          <DialogTitle>Add playlist</DialogTitle>
          <DialogContent className={styles.content}>
            {pending && (
              <>
                <div className={styles.preview}>
                  <Thumbnail src={entries[0]?.thumbnail ?? null} width={160} />
                  <div className={styles.previewText}>
                    <Text weight="semibold" className={styles.twoLines}>
                      {pending.info.title}
                    </Text>
                    <Caption1 className={styles.meta}>
                      {[pending.info.channel, `${entries.length} ${entries.length === 1 ? "video" : "videos"}`]
                        .filter(Boolean)
                        .join(" · ")}
                    </Caption1>
                  </div>
                </div>

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

                <div className={styles.choices}>
                  <Field label="Quality">
                    <Dropdown
                      value={choices.find((c) => c.value === quality)?.label ?? ""}
                      selectedOptions={[quality]}
                      onOptionSelect={(_, data) => data.optionValue && setQuality(data.optionValue)}
                    >
                      {choices.map((c) => (
                        <Option key={c.value} value={c.value}>
                          {c.label}
                        </Option>
                      ))}
                    </Dropdown>
                  </Field>
                  <Field label="Format">
                    <Dropdown
                      value={format.toUpperCase()}
                      selectedOptions={[format]}
                      onOptionSelect={(_, data) => data.optionValue && setFormat(data.optionValue)}
                    >
                      {formats.map((f) => (
                        <Option key={f} value={f} text={f.toUpperCase()}>
                          {f.toUpperCase()}
                        </Option>
                      ))}
                    </Dropdown>
                  </Field>
                </div>
              </>
            )}
          </DialogContent>
          <DialogActions>
            <Button appearance="secondary" onClick={() => pending && dismissPlaylist(pending.id)}>
              Cancel
            </Button>
            <Button
              className={styles.confirm}
              appearance="primary"
              icon={<AddRegular />}
              disabled={selected.size === 0}
              onClick={confirm}
            >
              Add {selected.size} {selected.size === 1 ? "video" : "videos"}
            </Button>
          </DialogActions>
        </DialogBody>
      </DialogSurface>
    </Dialog>
  );
}
