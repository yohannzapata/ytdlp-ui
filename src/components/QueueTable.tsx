import type { KeyboardEvent, ReactElement, ReactNode } from "react";
import {
  Body1,
  Button,
  Caption1,
  Dropdown,
  makeStyles,
  mergeClasses,
  Option,
  ProgressBar,
  shorthands,
  Spinner,
  Subtitle2,
  Text,
  tokens,
  Tooltip,
} from "@fluentui/react-components";
import {
  ArrowClockwiseRegular,
  ArrowDownload24Regular,
  CheckmarkCircleFilled,
  DismissRegular,
  ErrorCircleFilled,
  FolderOpenRegular,
  OpenRegular,
} from "@fluentui/react-icons";
import { api } from "../api";
import {
  AUDIO_FORMATS,
  formatBytes,
  formatDuration,
  formatEta,
  friendlyError,
  stageLabel,
  VIDEO_FORMATS,
} from "../format";
import { choicesFor, isActive, useApp, type Download } from "../store";
import { Thumbnail } from "./Thumbnail";

const COLUMNS = "minmax(200px, 1fr) 138px 88px 84px 210px 96px";

const useStyles = makeStyles({
  table: {
    flex: 1,
    minHeight: 0,
    overflow: "auto",
    display: "flex",
    flexDirection: "column",
    backgroundColor: tokens.colorNeutralBackground1,
    borderRadius: tokens.borderRadiusLarge,
    ...shorthands.border("1px", "solid", tokens.colorNeutralStroke2),
    position: "relative",
  },
  header: {
    position: "sticky",
    top: 0,
    zIndex: 1,
    flexShrink: 0,
    display: "grid",
    gridTemplateColumns: COLUMNS,
    columnGap: "12px",
    alignItems: "center",
    padding: "0 12px",
    height: "36px",
    backgroundColor: tokens.colorNeutralBackground2,
    color: tokens.colorNeutralForeground2,
    fontSize: tokens.fontSizeBase200,
    fontWeight: tokens.fontWeightSemibold,
    ...shorthands.borderBottom("1px", "solid", tokens.colorNeutralStroke2),
  },
  row: {
    display: "grid",
    gridTemplateColumns: COLUMNS,
    columnGap: "12px",
    alignItems: "center",
    flexShrink: 0,
    padding: "8px 12px",
    minHeight: "56px",
    boxSizing: "border-box",
    cursor: "default",
    outlineStyle: "none",
    ...shorthands.borderBottom("1px", "solid", tokens.colorNeutralStroke3),
    ":hover": { backgroundColor: tokens.colorSubtleBackgroundHover },
    ":focus-visible": { boxShadow: `inset 0 0 0 2px ${tokens.colorStrokeFocus2}` },
  },
  selected: {
    backgroundColor: tokens.colorBrandBackground2,
    ":hover": { backgroundColor: tokens.colorBrandBackground2Hover },
  },
  titleCell: { display: "flex", alignItems: "center", gap: "12px", minWidth: 0 },
  titleText: { display: "flex", flexDirection: "column", minWidth: 0 },
  muted: { color: tokens.colorNeutralForeground3 },
  choice: { minWidth: 0, width: "100%" },
  choiceText: { color: tokens.colorNeutralForeground2 },
  optionRow: { display: "flex", justifyContent: "space-between", gap: "16px", width: "100%" },
  status: { display: "flex", flexDirection: "column", gap: "4px", minWidth: 0 },
  statusLine: {
    display: "flex",
    alignItems: "center",
    gap: "6px",
    minWidth: 0,
    color: tokens.colorNeutralForeground2,
  },
  ellipsis: { overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" },
  ok: { color: tokens.colorPaletteGreenForeground1, flexShrink: 0 },
  bad: { color: tokens.colorPaletteRedForeground1, flexShrink: 0 },
  actions: { display: "flex", justifyContent: "flex-end", gap: "0" },
  empty: {
    flex: 1,
    minHeight: "160px",
    display: "flex",
    flexDirection: "column",
    alignItems: "center",
    justifyContent: "center",
    textAlign: "center",
    gap: "8px",
    padding: "24px",
  },
  emptyIcon: {
    width: "64px",
    height: "64px",
    borderRadius: "50%",
    display: "flex",
    alignItems: "center",
    justifyContent: "center",
    marginBottom: "8px",
    backgroundColor: tokens.colorBrandBackground2,
    color: tokens.colorBrandForeground2,
  },
  emptyText: { color: tokens.colorNeutralForeground3, maxWidth: "380px" },
});

function progressText(d: Download): string {
  if (d.percent == null) return "Starting…";
  const parts = [
    d.parts > 1 ? (d.part === 0 ? "Video" : "Audio") : null,
    `${Math.floor(d.percent)}%`,
    d.speed ? `${formatBytes(d.speed)}/s` : null,
    formatEta(d.eta) || null,
  ];
  return parts.filter(Boolean).join(" · ");
}

function StatusCell({ d }: { d: Download }) {
  const styles = useStyles();
  switch (d.status) {
    case "fetching":
      return (
        <Caption1 className={styles.statusLine}>
          <Spinner size="extra-tiny" /> Getting info…
        </Caption1>
      );
    case "ready":
      return <Caption1 className={styles.muted}>Ready</Caption1>;
    case "queued":
      return <Caption1 className={styles.muted}>Waiting…</Caption1>;
    case "downloading":
      return (
        <div className={styles.status}>
          <ProgressBar value={d.percent == null ? undefined : d.percent / 100} />
          <Caption1 className={mergeClasses(styles.statusLine, styles.ellipsis)}>{progressText(d)}</Caption1>
        </div>
      );
    case "processing":
      return (
        <div className={styles.status}>
          <ProgressBar />
          <Caption1 className={mergeClasses(styles.statusLine, styles.ellipsis)}>{stageLabel(d.stage ?? undefined)}</Caption1>
        </div>
      );
    case "done":
      return (
        <Caption1 className={styles.statusLine}>
          <CheckmarkCircleFilled className={styles.ok} fontSize={16} /> Done
        </Caption1>
      );
    case "error":
      return (
        <Caption1 className={styles.statusLine} title={d.error ?? undefined}>
          <ErrorCircleFilled className={styles.bad} fontSize={16} />
          <span className={styles.ellipsis}>{friendlyError(d.error)}</span>
        </Caption1>
      );
    case "canceled":
      return <Caption1 className={styles.muted}>Canceled</Caption1>;
  }
}

function IconButton({ label, icon, onClick }: { label: string; icon: ReactElement; onClick: () => void }) {
  return (
    <Tooltip content={label} relationship="label">
      <Button
        appearance="subtle"
        size="small"
        icon={icon}
        onClick={(e) => {
          e.stopPropagation();
          onClick();
        }}
      />
    </Tooltip>
  );
}

function Row({ d, selected }: { d: Download; selected: boolean }) {
  const styles = useStyles();
  const { select, setChoice, cancel, retry, remove } = useApp.getState();
  const audio = d.quality === "audio";
  const choices = choicesFor(d);
  const current = choices.find((c) => c.value === d.quality) ?? choices[0];
  const formats = audio ? AUDIO_FORMATS : VIDEO_FORMATS;
  const editable = d.status === "ready" || d.status === "error" || d.status === "canceled";
  const subtitle =
    d.needsInfo && d.status !== "ready"
      ? d.url
      : (d.source === "file"
          ? ["Local file", d.channel, formatDuration(d.duration), d.fileSize ? formatBytes(d.fileSize) : null]
          : [d.channel, formatDuration(d.duration)]
        )
          .filter(Boolean)
          .join(" · ");

  const openFile = () => d.filepath && api.openFile(d.filepath);
  const onKeyDown = (e: KeyboardEvent) => {
    if (e.target !== e.currentTarget) return;
    if (e.key === "Delete") remove(d.id);
    else if (e.key === "Enter" && d.status === "done") openFile();
  };

  let choiceCell: ReactNode[];
  if (d.needsInfo && d.status !== "ready") {
    choiceCell = [<span key="q" />, <span key="f" />];
  } else if (editable) {
    choiceCell = [
      <Dropdown
        key="q"
        size="small"
        className={styles.choice}
        value={current.label}
        selectedOptions={[current.value]}
        onOptionSelect={(_, data) => data.optionValue && setChoice(d.id, { quality: data.optionValue })}
      >
        {choices.map((c) => (
          <Option key={c.value} value={c.value} text={c.label}>
            <span className={styles.optionRow}>
              <span>{c.label}</span>
              {c.size ? <span className={styles.muted}>~{formatBytes(c.size)}</span> : null}
            </span>
          </Option>
        ))}
      </Dropdown>,
      <Dropdown
        key="f"
        size="small"
        className={styles.choice}
        value={d.format.toUpperCase()}
        selectedOptions={[d.format]}
        onOptionSelect={(_, data) => data.optionValue && setChoice(d.id, { format: data.optionValue })}
      >
        {formats.map((f) => (
          <Option key={f} value={f} text={f.toUpperCase()}>
            {f.toUpperCase()}
          </Option>
        ))}
      </Dropdown>,
    ];
  } else {
    choiceCell = [
      <Caption1 key="q" className={styles.choiceText}>
        {current.label.replace(/^Best \((.*)\)$/, "$1")}
      </Caption1>,
      <Caption1 key="f" className={styles.choiceText}>
        {d.format.toUpperCase()}
      </Caption1>,
    ];
  }

  return (
    <div
      role="row"
      aria-selected={selected}
      tabIndex={0}
      className={mergeClasses(styles.row, selected && styles.selected)}
      onClick={() => select(d.id)}
      onFocus={(e) => e.target === e.currentTarget && select(d.id)}
      onDoubleClick={openFile}
      onKeyDown={onKeyDown}
    >
      <div role="cell" className={styles.titleCell}>
        <Thumbnail src={d.thumbnail} width={64} audio={audio} />
        <div className={styles.titleText}>
          <Text weight="semibold" truncate wrap={false} title={d.title}>
            {d.title}
          </Text>
          <Caption1 className={mergeClasses(styles.muted, styles.ellipsis)}>{subtitle}</Caption1>
        </div>
      </div>
      <div role="cell">{choiceCell[0]}</div>
      <div role="cell">{choiceCell[1]}</div>
      <div role="cell">
        <Caption1 className={styles.muted}>{current.size && !d.needsInfo ? `~${formatBytes(current.size)}` : ""}</Caption1>
      </div>
      <div role="cell">
        <StatusCell d={d} />
      </div>
      <div role="cell" className={styles.actions}>
        {d.status === "done" && d.filepath && (
          <>
            <IconButton label="Open" icon={<OpenRegular />} onClick={openFile} />
            <IconButton label="Show in folder" icon={<FolderOpenRegular />} onClick={() => api.showInFolder(d.filepath!)} />
          </>
        )}
        {(d.status === "error" || d.status === "canceled") && (
          <IconButton label="Try again" icon={<ArrowClockwiseRegular />} onClick={() => retry(d.id)} />
        )}
        {isActive(d) || d.status === "fetching" ? (
          <IconButton label="Cancel" icon={<DismissRegular />} onClick={() => (d.status === "fetching" ? remove(d.id) : cancel(d.id))} />
        ) : (
          <IconButton label="Remove from list" icon={<DismissRegular />} onClick={() => remove(d.id)} />
        )}
      </div>
    </div>
  );
}

export function QueueTable() {
  const styles = useStyles();
  const downloads = useApp((s) => s.downloads);
  const selectedId = useApp((s) => s.selectedId);

  return (
    <div className={styles.table} role="table" aria-label="Downloads">
      <div className={styles.header} role="row">
        <span role="columnheader">Title</span>
        <span role="columnheader">Quality</span>
        <span role="columnheader">Format</span>
        <span role="columnheader">Size</span>
        <span role="columnheader">Status</span>
        <span role="columnheader" />
      </div>
      {downloads.length === 0 ? (
        <div className={styles.empty}>
          <div className={styles.emptyIcon}>
            <ArrowDownload24Regular />
          </div>
          <Subtitle2>No downloads yet</Subtitle2>
          <Body1 className={styles.emptyText}>Paste a link above, or drop video and audio files here.</Body1>
        </div>
      ) : (
        downloads.map((d) => <Row key={d.id} d={d} selected={d.id === selectedId} />)
      )}
    </div>
  );
}
