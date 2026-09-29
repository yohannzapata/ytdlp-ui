import { useEffect, useRef, useState } from "react";
import { Button, Caption1, makeStyles, shorthands, tokens } from "@fluentui/react-components";
import { CheckmarkRegular, CopyRegular } from "@fluentui/react-icons";
import { useApp } from "../store";

const useStyles = makeStyles({
  root: { height: "100%", display: "flex", flexDirection: "column", gap: "8px", minHeight: 0 },
  bar: { display: "flex", alignItems: "center", gap: "8px", minHeight: "24px" },
  title: { flex: 1, minWidth: 0, color: tokens.colorNeutralForeground3, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" },
  log: {
    flex: 1,
    minHeight: 0,
    margin: 0,
    overflow: "auto",
    padding: "8px 12px",
    borderRadius: tokens.borderRadiusMedium,
    backgroundColor: tokens.colorNeutralBackground1,
    ...shorthands.border("1px", "solid", tokens.colorNeutralStroke2),
    fontFamily: tokens.fontFamilyMonospace,
    fontSize: tokens.fontSizeBase200,
    lineHeight: tokens.lineHeightBase200,
    whiteSpace: "pre-wrap",
    wordBreak: "break-all",
    userSelect: "text",
  },
  hint: { color: tokens.colorNeutralForeground3 },
});

/** What yt-dlp printed for the selected download. */
export function OutputPanel() {
  const styles = useStyles();
  // The selected download, or else the most recent one that has printed something.
  const selected = useApp(
    (s) => s.downloads.find((d) => d.id === s.selectedId) ?? [...s.downloads].reverse().find((d) => d.log.length > 0),
  );
  const box = useRef<HTMLPreElement>(null);
  const [copied, setCopied] = useState(false);
  const lines = selected?.log.length ?? 0;

  useEffect(() => {
    const el = box.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [lines, selected?.id]);

  const copy = async () => {
    if (!selected) return;
    try {
      await navigator.clipboard.writeText(selected.log.join("\n"));
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      /* clipboard unavailable */
    }
  };

  return (
    <div className={styles.root}>
      <div className={styles.bar}>
        <Caption1 className={styles.title}>{selected ? selected.title : "Select a download to see what yt-dlp printed for it."}</Caption1>
        <Button
          size="small"
          appearance="subtle"
          icon={copied ? <CheckmarkRegular /> : <CopyRegular />}
          disabled={!lines}
          onClick={copy}
        >
          {copied ? "Copied" : "Copy"}
        </Button>
      </div>
      <pre ref={box} className={styles.log}>
        {selected ? (lines ? selected.log.join("\n") : "Nothing yet. Output appears here while it downloads.") : ""}
      </pre>
    </div>
  );
}
