import { useState } from "react";
import {
  Body1,
  Button,
  Caption1,
  Input,
  makeStyles,
  Subtitle2,
  tokens,
  Tooltip,
} from "@fluentui/react-components";
import { ArrowDownload24Regular, ArrowDownloadRegular, LinkRegular, SettingsRegular } from "@fluentui/react-icons";
import { isActive, isFinished, useApp } from "../store";
import { AddDialog } from "./AddDialog";
import { DownloadItem } from "./DownloadItem";
import { SettingsDialog } from "./SettingsDialog";

const CONTENT_WIDTH = "880px";

const useStyles = makeStyles({
  page: {
    height: "100%",
    display: "flex",
    flexDirection: "column",
  },
  top: {
    width: "100%",
    maxWidth: CONTENT_WIDTH,
    margin: "0 auto",
    padding: "24px 24px 0",
    boxSizing: "border-box",
  },
  bar: {
    display: "flex",
    gap: "8px",
  },
  url: { flex: 1, minWidth: 0 },
  header: {
    display: "flex",
    alignItems: "baseline",
    gap: "12px",
    margin: "28px 0 12px",
  },
  summary: { color: tokens.colorNeutralForeground3, flex: 1 },
  scroll: {
    flex: 1,
    overflowY: "auto",
  },
  list: {
    maxWidth: CONTENT_WIDTH,
    margin: "0 auto",
    padding: "0 24px 24px",
    boxSizing: "border-box",
    display: "flex",
    flexDirection: "column",
    gap: "8px",
  },
  empty: {
    display: "flex",
    flexDirection: "column",
    alignItems: "center",
    textAlign: "center",
    gap: "8px",
    padding: "64px 24px",
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
  emptyText: { color: tokens.colorNeutralForeground3, maxWidth: "360px" },
});

export function Home() {
  const styles = useStyles();
  const downloads = useApp((s) => s.downloads);
  const clearFinished = useApp((s) => s.clearFinished);
  const [url, setUrl] = useState("");
  const [pendingUrl, setPendingUrl] = useState<string | null>(null);
  const [settingsOpen, setSettingsOpen] = useState(false);

  const submit = () => {
    const link = url.trim();
    if (link) setPendingUrl(link);
  };

  const active = downloads.filter(isActive).length;
  const waiting = downloads.filter((d) => d.status === "queued").length;
  const finished = downloads.filter(isFinished).length;
  const summary = [active && `${active} downloading`, waiting && `${waiting} waiting`].filter(Boolean).join(" · ");

  return (
    <div className={styles.page}>
      <div className={styles.top}>
        <div className={styles.bar}>
          <Input
            className={styles.url}
            size="large"
            contentBefore={<LinkRegular />}
            placeholder="Paste a video or playlist link"
            value={url}
            onChange={(_, data) => setUrl(data.value)}
            onKeyDown={(e) => e.key === "Enter" && submit()}
            autoFocus
          />
          <Button appearance="primary" size="large" icon={<ArrowDownloadRegular />} disabled={!url.trim()} onClick={submit}>
            Download
          </Button>
          <Tooltip content="Settings" relationship="label">
            <Button appearance="subtle" size="large" icon={<SettingsRegular />} onClick={() => setSettingsOpen(true)} />
          </Tooltip>
        </div>

        <div className={styles.header}>
          <Subtitle2>Downloads</Subtitle2>
          <Caption1 className={styles.summary}>{summary}</Caption1>
          {finished > 0 && (
            <Button appearance="subtle" size="small" onClick={clearFinished}>
              Clear finished
            </Button>
          )}
        </div>
      </div>

      <div className={styles.scroll}>
        <div className={styles.list}>
          {downloads.length === 0 ? (
            <div className={styles.empty}>
              <div className={styles.emptyIcon}>
                <ArrowDownload24Regular />
              </div>
              <Subtitle2>No downloads yet</Subtitle2>
              <Body1 className={styles.emptyText}>
                Paste a link from YouTube or one of the many other sites yt-dlp supports to get started.
              </Body1>
            </div>
          ) : (
            downloads.map((d) => <DownloadItem key={d.id} d={d} />)
          )}
        </div>
      </div>

      <AddDialog
        url={pendingUrl}
        onClose={() => setPendingUrl(null)}
        onAdded={() => {
          setPendingUrl(null);
          setUrl("");
        }}
      />
      <SettingsDialog open={settingsOpen} onClose={() => setSettingsOpen(false)} />
    </div>
  );
}
