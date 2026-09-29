import { useEffect, useState, type ReactNode } from "react";
import {
  Body1,
  Button,
  Caption1,
  Dialog,
  DialogBody,
  DialogContent,
  DialogSurface,
  DialogTitle,
  DialogTrigger,
  Dropdown,
  makeStyles,
  Option,
  shorthands,
  Switch,
  tokens,
} from "@fluentui/react-components";
import {
  ArrowDownloadRegular,
  ArrowSyncCheckmarkRegular,
  ArrowSyncRegular,
  DarkThemeRegular,
  DismissRegular,
  FolderRegular,
} from "@fluentui/react-icons";
import { getVersion } from "@tauri-apps/api/app";
import { api, chooseFolder, type Theme } from "../api";
import { useApp } from "../store";

const useStyles = makeStyles({
  surface: { maxWidth: "620px" },
  content: {
    display: "flex",
    flexDirection: "column",
    gap: "4px",
  },
  row: {
    display: "flex",
    alignItems: "center",
    gap: "16px",
    minHeight: "68px",
    boxSizing: "border-box",
    padding: "12px 16px",
    borderRadius: tokens.borderRadiusMedium,
    backgroundColor: tokens.colorNeutralBackground2,
    ...shorthands.border("1px", "solid", tokens.colorNeutralStroke3),
  },
  icon: {
    fontSize: "20px",
    display: "flex",
    color: tokens.colorNeutralForeground2,
  },
  text: {
    flex: 1,
    minWidth: 0,
    display: "flex",
    flexDirection: "column",
  },
  description: {
    color: tokens.colorNeutralForeground3,
    overflow: "hidden",
    textOverflow: "ellipsis",
    whiteSpace: "nowrap",
  },
  control: { flexShrink: 0 },
  dropdown: { minWidth: "150px" },
  footer: {
    marginTop: "12px",
    color: tokens.colorNeutralForeground3,
    display: "flex",
    flexDirection: "column",
    gap: "2px",
  },
});

function Row({ icon, title, description, children }: { icon: ReactNode; title: string; description?: string; children: ReactNode }) {
  const styles = useStyles();
  return (
    <div className={styles.row}>
      <span className={styles.icon}>{icon}</span>
      <div className={styles.text}>
        <Body1>{title}</Body1>
        {description && (
          <Caption1 className={styles.description} title={description}>
            {description}
          </Caption1>
        )}
      </div>
      <div className={styles.control}>{children}</div>
    </div>
  );
}

const THEMES: Record<Theme, string> = { system: "Use system setting", light: "Light", dark: "Dark" };

export function SettingsDialog({ open, onClose }: { open: boolean; onClose(): void }) {
  const styles = useStyles();
  const settings = useApp((s) => s.settings)!;
  const tools = useApp((s) => s.tools) ?? [];
  const { updateSettings, refreshTools } = useApp.getState();
  const [checking, setChecking] = useState(false);
  const [updateMessage, setUpdateMessage] = useState<string | null>(null);
  const [appVersion, setAppVersion] = useState("");

  useEffect(() => {
    getVersion().then(setAppVersion).catch(() => {});
  }, []);

  const version = (tool: string) => tools.find((t) => t.tool === tool)?.version ?? "unknown";

  const checkForUpdates = async () => {
    setChecking(true);
    setUpdateMessage(null);
    const before = version("ytdlp");
    try {
      const after = await api.updateYtdlp();
      await refreshTools();
      setUpdateMessage(after === before ? "You're up to date" : `Updated to ${after}`);
      updateSettings({ lastUpdateCheck: Math.floor(Date.now() / 1000) });
    } catch (err) {
      setUpdateMessage(`Couldn't update: ${err}`);
    } finally {
      setChecking(false);
    }
  };

  const changeFolder = async () => {
    const folder = await chooseFolder(settings.downloadDir);
    if (folder) updateSettings({ downloadDir: folder });
  };

  return (
    <Dialog open={open} onOpenChange={(_, data) => !data.open && onClose()}>
      <DialogSurface className={styles.surface}>
        <DialogBody>
          <DialogTitle
            action={
              <DialogTrigger action="close">
                <Button appearance="subtle" aria-label="Close" icon={<DismissRegular />} />
              </DialogTrigger>
            }
          >
            Settings
          </DialogTitle>
          <DialogContent className={styles.content}>
            <Row icon={<FolderRegular />} title="Download folder" description={settings.downloadDir}>
              <Button onClick={changeFolder}>Change…</Button>
            </Row>

            <Row icon={<ArrowDownloadRegular />} title="Downloads at the same time">
              <Dropdown
                className={styles.dropdown}
                value={String(settings.maxConcurrent)}
                selectedOptions={[String(settings.maxConcurrent)]}
                onOptionSelect={(_, data) => updateSettings({ maxConcurrent: Number(data.optionValue) })}
              >
                {[1, 2, 3, 4, 5].map((n) => (
                  <Option key={n} value={String(n)}>
                    {String(n)}
                  </Option>
                ))}
              </Dropdown>
            </Row>

            <Row icon={<DarkThemeRegular />} title="App theme">
              <Dropdown
                className={styles.dropdown}
                value={THEMES[settings.theme]}
                selectedOptions={[settings.theme]}
                onOptionSelect={(_, data) => updateSettings({ theme: data.optionValue as Theme })}
              >
                {Object.entries(THEMES).map(([value, label]) => (
                  <Option key={value} value={value}>
                    {label}
                  </Option>
                ))}
              </Dropdown>
            </Row>

            <Row
              icon={<ArrowSyncRegular />}
              title="yt-dlp"
              description={updateMessage ?? `Version ${version("ytdlp")}`}
            >
              <Button onClick={checkForUpdates} disabled={checking}>
                {checking ? "Checking…" : "Check for updates"}
              </Button>
            </Row>

            <Row
              icon={<ArrowSyncCheckmarkRegular />}
              title="Update yt-dlp automatically"
              description="Keeps downloads working when websites change"
            >
              <Switch
                checked={settings.autoUpdate}
                onChange={(_, data) => updateSettings({ autoUpdate: data.checked })}
              />
            </Row>

            <div className={styles.footer}>
              <Caption1>
                FFmpeg {version("ffmpeg")} · Deno {version("deno")}
              </Caption1>
              <Caption1>ytdlp-ui {appVersion} · Powered by yt-dlp</Caption1>
            </div>
          </DialogContent>
        </DialogBody>
      </DialogSurface>
    </Dialog>
  );
}
