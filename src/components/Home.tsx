import { useEffect, useState } from "react";
import {
  Button,
  Caption1,
  Input,
  makeStyles,
  Menu,
  MenuDivider,
  MenuItem,
  MenuList,
  MenuPopover,
  MenuTrigger,
  Tab,
  TabList,
  shorthands,
  tokens,
  Tooltip,
} from "@fluentui/react-components";
import {
  AddRegular,
  ArrowClockwiseRegular,
  ChevronDownRegular,
  ChevronUpRegular,
  DeleteRegular,
  FolderOpenRegular,
  LinkRegular,
  MoreHorizontalRegular,
  PlayFilled,
  SettingsRegular,
  StopFilled,
} from "@fluentui/react-icons";
import { api } from "../api";
import { findLinks, isActive, isFinished, useApp } from "../store";
import { OptionsPanel } from "./OptionsPanel";
import { OutputPanel } from "./OutputPanel";
import { PlaylistDialog } from "./PlaylistDialog";
import { QueueTable } from "./QueueTable";
import { SettingsDialog } from "./SettingsDialog";

const PANEL_HEIGHT = "224px";

const useStyles = makeStyles({
  page: {
    height: "100%",
    display: "flex",
    flexDirection: "column",
    gap: "12px",
    padding: "16px",
    boxSizing: "border-box",
  },
  top: { display: "flex", gap: "8px", flexShrink: 0 },
  url: { flex: 1, minWidth: 0 },
  bar: { display: "flex", alignItems: "center", gap: "8px", flexShrink: 0 },
  summary: { marginLeft: "auto", color: tokens.colorNeutralForeground3 },
  panel: {
    flexShrink: 0,
    borderRadius: tokens.borderRadiusLarge,
    backgroundColor: tokens.colorNeutralBackground2,
    ...shorthands.border("1px", "solid", tokens.colorNeutralStroke2),
    overflow: "hidden",
  },
  tabs: { display: "flex", alignItems: "center", padding: "0 4px 0 8px", justifyContent: "space-between" },
  panelBody: { height: PANEL_HEIGHT, boxSizing: "border-box", padding: "4px 16px 14px", overflowY: "auto" },
});

export function Home() {
  const styles = useStyles();
  const downloads = useApp((s) => s.downloads);
  const settings = useApp((s) => s.settings)!;
  const { addLinks, start, stopAll, retryFailed, clearFinished, clearAll, updateSettings } = useApp.getState();
  const [link, setLink] = useState("");
  const [settingsOpen, setSettingsOpen] = useState(false);

  // Ctrl+V anywhere in the window adds the copied link(s).
  useEffect(() => {
    const onPaste = (e: ClipboardEvent) => {
      if ((e.target as HTMLElement | null)?.closest("input, textarea")) return;
      if (useApp.getState().addLinks(e.clipboardData?.getData("text") ?? "")) e.preventDefault();
    };
    document.addEventListener("paste", onPaste);
    return () => document.removeEventListener("paste", onPaste);
  }, []);

  const submit = () => {
    if (addLinks(link)) setLink("");
  };

  const ready = downloads.filter((d) => d.status === "ready").length;
  const running = downloads.filter(isActive).length;
  const waiting = downloads.filter((d) => d.status === "queued").length;
  const finished = downloads.filter(isFinished).length;
  const failed = downloads.filter((d) => d.status === "error" || d.status === "canceled").length;
  const summary = [running && `${running} downloading`, waiting && `${waiting} waiting`, ready && `${ready} ready`]
    .filter(Boolean)
    .join(" · ");

  const openPanel = (tab: "options" | "output") => updateSettings({ panelTab: tab, panelOpen: true });

  return (
    <div className={styles.page}>
      <div className={styles.top}>
        <Input
          className={styles.url}
          size="large"
          contentBefore={<LinkRegular />}
          placeholder="Paste a video or playlist link"
          value={link}
          onChange={(_, data) => setLink(data.value)}
          onKeyDown={(e) => e.key === "Enter" && submit()}
          onPaste={(e) => {
            // Pasting a link adds it straight away, several links at once too.
            const text = e.clipboardData.getData("text");
            if (findLinks(text).length && addLinks(text)) {
              e.preventDefault();
              setLink("");
            }
          }}
          autoFocus
        />
        <Button size="large" icon={<AddRegular />} disabled={!link.trim()} onClick={submit}>
          Add
        </Button>
        <Tooltip content="Settings" relationship="label">
          <Button appearance="subtle" size="large" icon={<SettingsRegular />} onClick={() => setSettingsOpen(true)} />
        </Tooltip>
      </div>

      <QueueTable />

      <div className={styles.bar}>
        <Button appearance="primary" icon={<PlayFilled />} disabled={ready === 0} onClick={start}>
          {ready ? `Start (${ready})` : "Start"}
        </Button>
        <Button icon={<StopFilled />} disabled={running + waiting === 0} onClick={stopAll}>
          Stop
        </Button>
        <Menu>
          <MenuTrigger disableButtonEnhancement>
            <Button icon={<MoreHorizontalRegular />}>Queue actions</Button>
          </MenuTrigger>
          <MenuPopover>
            <MenuList>
              <MenuItem icon={<ArrowClockwiseRegular />} disabled={failed === 0} onClick={retryFailed}>
                Retry failed
              </MenuItem>
              <MenuItem icon={<DeleteRegular />} disabled={finished === 0} onClick={clearFinished}>
                Remove finished
              </MenuItem>
              <MenuItem icon={<DeleteRegular />} disabled={downloads.length === 0} onClick={clearAll}>
                Remove all
              </MenuItem>
              <MenuDivider />
              <MenuItem icon={<FolderOpenRegular />} onClick={() => api.openFile(settings.downloadDir).catch(console.error)}>
                Open download folder
              </MenuItem>
            </MenuList>
          </MenuPopover>
        </Menu>
        <Caption1 className={styles.summary}>{summary}</Caption1>
      </div>

      <div className={styles.panel}>
        <div className={styles.tabs}>
          <TabList
            size="small"
            selectedValue={settings.panelTab}
            onTabSelect={(_, data) => openPanel(data.value as "options" | "output")}
          >
            <Tab value="options">Options</Tab>
            <Tab value="output">Output</Tab>
          </TabList>
          <Tooltip content={settings.panelOpen ? "Hide" : "Show"} relationship="label">
            <Button
              appearance="subtle"
              size="small"
              icon={settings.panelOpen ? <ChevronDownRegular /> : <ChevronUpRegular />}
              onClick={() => updateSettings({ panelOpen: !settings.panelOpen })}
            />
          </Tooltip>
        </div>
        {settings.panelOpen && (
          <div className={styles.panelBody}>{settings.panelTab === "options" ? <OptionsPanel /> : <OutputPanel />}</div>
        )}
      </div>

      <PlaylistDialog />
      <SettingsDialog open={settingsOpen} onClose={() => setSettingsOpen(false)} />
    </div>
  );
}
