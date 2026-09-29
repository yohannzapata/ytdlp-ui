import { useEffect, useState } from "react";
import {
  Body1,
  Button,
  Caption1,
  makeStyles,
  MessageBar,
  MessageBarBody,
  ProgressBar,
  shorthands,
  Text,
  Title3,
  tokens,
} from "@fluentui/react-components";
import { CheckmarkCircleFilled, ErrorCircleFilled } from "@fluentui/react-icons";
import { api, onInstallProgress, type InstallProgress, type Tool } from "../api";
import { formatBytes } from "../format";
import { useApp } from "../store";

const useStyles = makeStyles({
  page: {
    height: "100%",
    display: "flex",
    alignItems: "center",
    justifyContent: "center",
    padding: "24px",
    boxSizing: "border-box",
  },
  card: {
    width: "100%",
    maxWidth: "520px",
    display: "flex",
    flexDirection: "column",
    gap: "16px",
    padding: "28px",
    borderRadius: tokens.borderRadiusXLarge,
    backgroundColor: tokens.colorNeutralBackground1,
    boxShadow: tokens.shadow8,
  },
  intro: { color: tokens.colorNeutralForeground2 },
  tools: {
    display: "flex",
    flexDirection: "column",
    borderRadius: tokens.borderRadiusMedium,
    ...shorthands.border("1px", "solid", tokens.colorNeutralStroke2),
  },
  tool: {
    display: "flex",
    alignItems: "center",
    gap: "16px",
    padding: "12px 16px",
    ":not(:last-child)": shorthands.borderBottom("1px", "solid", tokens.colorNeutralStroke2),
  },
  toolText: {
    flex: 1,
    display: "flex",
    flexDirection: "column",
  },
  muted: { color: tokens.colorNeutralForeground3 },
  status: {
    width: "150px",
    display: "flex",
    flexDirection: "column",
    alignItems: "flex-end",
    gap: "4px",
    color: tokens.colorNeutralForeground2,
  },
  statusLine: { display: "flex", alignItems: "center", gap: "6px" },
  ok: { color: tokens.colorPaletteGreenForeground1 },
  bad: { color: tokens.colorPaletteRedForeground1 },
  actions: { display: "flex", justifyContent: "flex-end" },
});

const TOOLS: { tool: Tool; name: string; description: string }[] = [
  { tool: "ytdlp", name: "yt-dlp", description: "Downloads the videos" },
  { tool: "ffmpeg", name: "FFmpeg", description: "Joins video and audio, converts to MP3" },
  { tool: "deno", name: "Deno", description: "Needed for YouTube" },
];

export function Setup() {
  const styles = useStyles();
  const tools = useApp((s) => s.tools) ?? [];
  const [progress, setProgress] = useState<Partial<Record<Tool, InstallProgress>>>({});
  const [installing, setInstalling] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const unlisten = onInstallProgress((p) => setProgress((prev) => ({ ...prev, [p.tool]: p })));
    return () => {
      unlisten.then((stop) => stop());
    };
  }, []);

  const install = async () => {
    setInstalling(true);
    setError(null);
    setProgress({});
    try {
      await api.toolsInstall();
    } catch (err) {
      setError(String(err));
    }
    await useApp.getState().refreshTools();
    setInstalling(false);
  };

  const renderStatus = (tool: Tool) => {
    const p = progress[tool];
    if (tools.find((t) => t.tool === tool)?.found || p?.phase === "done") {
      return (
        <Caption1 className={styles.statusLine}>
          <CheckmarkCircleFilled className={styles.ok} fontSize={16} /> Ready
        </Caption1>
      );
    }
    if (p?.phase === "error") {
      return (
        <Caption1 className={styles.statusLine}>
          <ErrorCircleFilled className={styles.bad} fontSize={16} /> Failed
        </Caption1>
      );
    }
    if (p?.phase === "downloading") {
      return (
        <>
          <ProgressBar value={p.total ? p.received / p.total : undefined} />
          <Caption1>{p.total ? `${formatBytes(p.received)} of ${formatBytes(p.total)}` : formatBytes(p.received)}</Caption1>
        </>
      );
    }
    if (p?.phase === "extracting") {
      return (
        <>
          <ProgressBar />
          <Caption1>Unpacking…</Caption1>
        </>
      );
    }
    return <Caption1 className={styles.muted}>{installing ? "Waiting…" : "Not installed"}</Caption1>;
  };

  return (
    <div className={styles.page}>
      <div className={styles.card}>
        <Title3>Welcome to ytdlp-ui</Title3>
        <Body1 className={styles.intro}>
          Before your first download, ytdlp-ui needs a few free tools. They're downloaded once from their official
          sources.
        </Body1>

        <div className={styles.tools}>
          {TOOLS.map(({ tool, name, description }) => (
            <div key={tool} className={styles.tool}>
              <div className={styles.toolText}>
                <Text weight="semibold">{name}</Text>
                <Caption1 className={styles.muted}>{description}</Caption1>
              </div>
              <div className={styles.status}>{renderStatus(tool)}</div>
            </div>
          ))}
        </div>

        {error && (
          <MessageBar intent="error" layout="multiline">
            <MessageBarBody>{error}</MessageBarBody>
          </MessageBar>
        )}

        <div className={styles.actions}>
          <Button appearance="primary" size="large" onClick={install} disabled={installing}>
            {installing ? "Installing…" : error ? "Try again" : "Install"}
          </Button>
        </div>
      </div>
    </div>
  );
}
