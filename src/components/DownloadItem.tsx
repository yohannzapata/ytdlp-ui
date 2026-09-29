import {
  Button,
  Caption1,
  makeStyles,
  ProgressBar,
  shorthands,
  Text,
  tokens,
  Tooltip,
} from "@fluentui/react-components";
import {
  ArrowClockwiseRegular,
  CheckmarkCircleFilled,
  DismissRegular,
  ErrorCircleFilled,
  FolderOpenRegular,
  OpenRegular,
} from "@fluentui/react-icons";
import type { ReactElement } from "react";
import { api } from "../api";
import { describeChoice, formatBytes, formatEta, friendlyError, stageLabel } from "../format";
import { isActive, useApp, type Download } from "../store";
import { Thumbnail } from "./Thumbnail";

const useStyles = makeStyles({
  card: {
    display: "flex",
    alignItems: "center",
    gap: "16px",
    padding: "12px",
    borderRadius: tokens.borderRadiusLarge,
    backgroundColor: tokens.colorNeutralBackground1,
    ...shorthands.border("1px", "solid", tokens.colorNeutralStroke2),
  },
  body: {
    flex: 1,
    minWidth: 0,
    display: "flex",
    flexDirection: "column",
    gap: "2px",
  },
  meta: {
    color: tokens.colorNeutralForeground3,
  },
  status: {
    display: "flex",
    flexDirection: "column",
    gap: "6px",
    marginTop: "6px",
  },
  statusLine: {
    display: "flex",
    alignItems: "center",
    gap: "6px",
    color: tokens.colorNeutralForeground2,
    minWidth: 0,
  },
  statusText: {
    overflow: "hidden",
    textOverflow: "ellipsis",
    whiteSpace: "nowrap",
  },
  ok: { color: tokens.colorPaletteGreenForeground1, flexShrink: 0 },
  bad: { color: tokens.colorPaletteRedForeground1, flexShrink: 0 },
  actions: {
    display: "flex",
    gap: "2px",
    flexShrink: 0,
  },
});

function progressText(d: Download): string {
  if (d.percent == null) return "Starting…";
  const parts = [
    d.parts > 1 ? (d.part === 0 ? "Video" : "Audio") : null,
    `${Math.floor(d.percent)}%`,
    d.total ? `${formatBytes(d.downloaded)} of ${formatBytes(d.total)}` : null,
    d.speed ? `${formatBytes(d.speed)}/s` : null,
    formatEta(d.eta) || null,
  ];
  return parts.filter(Boolean).join(" · ");
}

function Status({ d }: { d: Download }) {
  const styles = useStyles();
  switch (d.status) {
    case "queued":
      return <Caption1 className={styles.statusLine}>Waiting…</Caption1>;
    case "downloading":
      return (
        <div className={styles.status}>
          <ProgressBar value={d.percent == null ? undefined : d.percent / 100} />
          <Caption1 className={styles.statusLine}>{progressText(d)}</Caption1>
        </div>
      );
    case "processing":
      return (
        <div className={styles.status}>
          <ProgressBar />
          <Caption1 className={styles.statusLine}>{stageLabel(d.stage ?? undefined)}</Caption1>
        </div>
      );
    case "done":
      return (
        <Caption1 className={styles.statusLine}>
          <CheckmarkCircleFilled className={styles.ok} fontSize={16} />
          Done
        </Caption1>
      );
    case "error":
      return (
        <Caption1 className={styles.statusLine} title={d.error ?? undefined}>
          <ErrorCircleFilled className={styles.bad} fontSize={16} />
          <span className={styles.statusText}>{friendlyError(d.error)}</span>
        </Caption1>
      );
    case "canceled":
      return <Caption1 className={styles.statusLine}>Canceled</Caption1>;
  }
}

function Action({ label, icon, onClick }: { label: string; icon: ReactElement; onClick: () => void }) {
  return (
    <Tooltip content={label} relationship="label">
      <Button appearance="subtle" icon={icon} onClick={onClick} />
    </Tooltip>
  );
}

export function DownloadItem({ d }: { d: Download }) {
  const styles = useStyles();
  const { cancel, retry, remove } = useApp.getState();
  const meta = [d.channel, describeChoice(d.audioOnly, d.maxHeight, d.format)].filter(Boolean).join(" · ");

  return (
    <div className={styles.card}>
      <Thumbnail src={d.thumbnail} width={112} audio={d.audioOnly} />
      <div className={styles.body}>
        <Text weight="semibold" truncate wrap={false} block title={d.title}>
          {d.title}
        </Text>
        <Caption1 className={styles.meta} truncate wrap={false} block>
          {meta}
        </Caption1>
        <Status d={d} />
      </div>
      <div className={styles.actions}>
        {d.status === "done" && d.filepath && (
          <>
            <Action label="Open" icon={<OpenRegular />} onClick={() => api.openFile(d.filepath!)} />
            <Action label="Show in folder" icon={<FolderOpenRegular />} onClick={() => api.showInFolder(d.filepath!)} />
          </>
        )}
        {(d.status === "error" || d.status === "canceled") && (
          <Action label="Try again" icon={<ArrowClockwiseRegular />} onClick={() => retry(d.id)} />
        )}
        {isActive(d) ? (
          <Action label="Cancel" icon={<DismissRegular />} onClick={() => cancel(d.id)} />
        ) : (
          <Action label="Remove from list" icon={<DismissRegular />} onClick={() => remove(d.id)} />
        )}
      </div>
    </div>
  );
}
