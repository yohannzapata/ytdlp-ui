import { useEffect, useState } from "react";
import {
  FluentProvider,
  makeStyles,
  mergeClasses,
  Spinner,
  tokens,
  webDarkTheme,
  webLightTheme,
  type Theme,
} from "@fluentui/react-components";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { api, onDownloadEvent, onStemsProgress } from "./api";
import { Home } from "./components/Home";
import { Setup } from "./components/Setup";
import { useApp } from "./store";

const font = '"Segoe UI Variable Text", "Segoe UI", system-ui, -apple-system, BlinkMacSystemFont, sans-serif';
const lightTheme: Theme = { ...webLightTheme, fontFamilyBase: font };
const darkTheme: Theme = { ...webDarkTheme, fontFamilyBase: font };

const useStyles = makeStyles({
  root: { height: "100%" },
  light: { backgroundColor: tokens.colorNeutralBackground3 },
  dark: { backgroundColor: tokens.colorNeutralBackground2 },
  loading: { height: "100%" },
});

const darkQuery = window.matchMedia("(prefers-color-scheme: dark)");

function useSystemDark() {
  const [dark, setDark] = useState(darkQuery.matches);
  useEffect(() => {
    const onChange = (e: MediaQueryListEvent) => setDark(e.matches);
    darkQuery.addEventListener("change", onChange);
    return () => darkQuery.removeEventListener("change", onChange);
  }, []);
  return dark;
}

const DAY = 24 * 60 * 60;
let autoUpdateStarted = false;

export default function App() {
  const styles = useStyles();
  const settings = useApp((s) => s.settings);
  const tools = useApp((s) => s.tools);
  const systemDark = useSystemDark();
  const themeSetting = settings?.theme ?? "system";
  const dark = themeSetting === "dark" || (themeSetting === "system" && systemDark);
  const ready = tools?.every((t) => t.found) ?? false;

  useEffect(() => {
    useApp.getState().load().catch(console.error);
    const unlisten = onDownloadEvent((event) => useApp.getState().handleEvent(event));
    const unlistenStems = onStemsProgress((event) => useApp.getState().handleStemsEvent(event));
    return () => {
      unlisten.then((stop) => stop());
      unlistenStems.then((stop) => stop());
    };
  }, []);

  // Keep the window's title bar in the same theme as the app.
  useEffect(() => {
    getCurrentWindow()
      .setTheme(themeSetting === "system" ? null : themeSetting)
      .catch(() => {});
  }, [themeSetting]);

  // Sites change often; a fresh yt-dlp keeps downloads working. Checked at most once a day.
  useEffect(() => {
    if (!ready || !settings?.autoUpdate || autoUpdateStarted) return;
    const now = Math.floor(Date.now() / 1000);
    if (now - settings.lastUpdateCheck < DAY) return;
    autoUpdateStarted = true;
    useApp.getState().updateSettings({ lastUpdateCheck: now });
    api
      .updateYtdlp()
      .then(() => useApp.getState().refreshTools())
      .catch(console.warn);
  }, [ready, settings?.autoUpdate, settings?.lastUpdateCheck]);

  return (
    // Page styles go on an inner element: FluentProvider copies its own classes onto popups and dialogs.
    <FluentProvider theme={dark ? darkTheme : lightTheme} style={{ height: "100%" }}>
      <div className={mergeClasses(styles.root, dark ? styles.dark : styles.light)}>
        {!settings || !tools ? <Spinner className={styles.loading} /> : ready ? <Home /> : <Setup />}
      </div>
    </FluentProvider>
  );
}
