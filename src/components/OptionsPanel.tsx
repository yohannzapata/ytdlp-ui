import { Button, Checkbox, Dropdown, Field, Input, makeStyles, mergeClasses, Option, tokens, Tooltip } from "@fluentui/react-components";
import { FolderOpenRegular, FolderRegular } from "@fluentui/react-icons";
import { api, chooseFolder, type DownloadOptions } from "../api";
import { useApp } from "../store";

const useStyles = makeStyles({
  grid: {
    display: "grid",
    gridTemplateColumns: "repeat(3, minmax(0, 1fr))",
    gap: "10px 16px",
    alignItems: "start",
    "@media (max-width: 1040px)": { gridTemplateColumns: "repeat(2, minmax(0, 1fr))" },
  },
  wide: { gridColumn: "span 2" },
  full: { gridColumn: "1 / -1" },
  inline: { display: "flex", gap: "8px", minWidth: 0 },
  grow: { flex: 1, minWidth: 0 },
  unit: { minWidth: "96px", width: "96px" },
  checks: { display: "flex", flexWrap: "wrap", columnGap: "20px", rowGap: "0" },
  mono: { fontFamily: tokens.fontFamilyMonospace },
});

type Choice<T extends string> = { value: T; label: string };

const SUBTITLES: Choice<DownloadOptions["subtitles"]>[] = [
  { value: "off", label: "Don't include" },
  { value: "embed", label: "Embed in the video" },
  { value: "file", label: "Save as .srt file" },
];

const CHAPTERS: Choice<DownloadOptions["chapters"]>[] = [
  { value: "embed", label: "Embed in the file" },
  { value: "split", label: "Split into files" },
  { value: "ignore", label: "Ignore" },
];

const UNITS: Choice<DownloadOptions["rateLimitUnit"]>[] = [
  { value: "K", label: "KB/s" },
  { value: "M", label: "MB/s" },
];

const isMac = /Mac/i.test(navigator.platform);
const BROWSERS: Choice<string>[] = [
  { value: "", label: "Don't sign in" },
  { value: "firefox", label: "Firefox" },
  { value: "chrome", label: "Chrome" },
  { value: "edge", label: "Edge" },
  { value: "brave", label: "Brave" },
  { value: "chromium", label: "Chromium" },
  { value: "opera", label: "Opera" },
  { value: "vivaldi", label: "Vivaldi" },
  ...(isMac ? [{ value: "safari", label: "Safari" }] : []),
];

function Select<T extends string>({
  value,
  choices,
  onChange,
  className,
}: {
  value: T;
  choices: Choice<T>[];
  onChange: (value: T) => void;
  className?: string;
}) {
  return (
    <Dropdown
      size="small"
      className={className}
      value={choices.find((c) => c.value === value)?.label ?? ""}
      selectedOptions={[value]}
      onOptionSelect={(_, data) => data.optionValue !== undefined && onChange(data.optionValue as T)}
    >
      {choices.map((c) => (
        <Option key={c.value} value={c.value}>
          {c.label}
        </Option>
      ))}
    </Dropdown>
  );
}

export function OptionsPanel() {
  const styles = useStyles();
  const settings = useApp((s) => s.settings)!;
  const { updateSettings, updateOptions } = useApp.getState();
  const o = settings.options;

  const browse = async () => {
    const folder = await chooseFolder(settings.downloadDir);
    if (folder) updateSettings({ downloadDir: folder });
  };

  return (
    <div className={styles.grid}>
      <Field className={styles.wide} label="Save to" size="small">
        <div className={styles.inline}>
          <Input
            size="small"
            className={styles.grow}
            readOnly
            value={settings.downloadDir}
            title={settings.downloadDir}
            contentBefore={<FolderRegular />}
          />
          <Button size="small" onClick={browse}>
            Browse…
          </Button>
          <Tooltip content="Open folder" relationship="label">
            <Button
              size="small"
              icon={<FolderOpenRegular />}
              onClick={() => api.openFile(settings.downloadDir).catch(console.error)}
            />
          </Tooltip>
        </div>
      </Field>

      <Field label="Speed limit" size="small">
        <div className={styles.inline}>
          <Input
            size="small"
            className={styles.grow}
            inputMode="decimal"
            placeholder="No limit"
            value={o.rateLimitValue}
            onChange={(_, data) => updateOptions({ rateLimitValue: data.value.replace(/[^0-9.,]/g, "") })}
          />
          <Select
            className={styles.unit}
            value={o.rateLimitUnit}
            choices={UNITS}
            onChange={(rateLimitUnit) => updateOptions({ rateLimitUnit })}
          />
        </div>
      </Field>

      <Field label="Subtitles" size="small">
        <div className={styles.inline}>
          <Select
            className={styles.grow}
            value={o.subtitles}
            choices={SUBTITLES}
            onChange={(subtitles) => updateOptions({ subtitles })}
          />
          <Input
            size="small"
            style={{ width: "72px" }}
            aria-label="Subtitle languages"
            title="Languages, for example: en, de"
            disabled={o.subtitles === "off"}
            value={o.subtitleLangs}
            onChange={(_, data) => updateOptions({ subtitleLangs: data.value.replace(/\s+/g, "") })}
          />
        </div>
      </Field>

      <Field label="Chapters" size="small">
        <Select value={o.chapters} choices={CHAPTERS} onChange={(chapters) => updateOptions({ chapters })} />
      </Field>

      <Field label="Sign in with browser cookies" size="small" title="Lets yt-dlp use your browser's login, for age-restricted or members-only videos.">
        <Select
          value={o.cookiesBrowser}
          choices={BROWSERS}
          onChange={(cookiesBrowser) => updateOptions({ cookiesBrowser })}
        />
      </Field>

      <div className={mergeClasses(styles.checks, styles.full)}>
        <Checkbox
          label="Embed thumbnail"
          checked={o.embedThumbnail}
          onChange={(_, data) => updateOptions({ embedThumbnail: !!data.checked })}
        />
        <Checkbox
          label="Embed title and info"
          checked={o.embedMetadata}
          onChange={(_, data) => updateOptions({ embedMetadata: !!data.checked })}
        />
        <Checkbox
          label="Skip sponsor segments"
          checked={o.sponsorblock}
          onChange={(_, data) => updateOptions({ sponsorblock: !!data.checked })}
        />
        <Checkbox
          label="File date = download time"
          checked={o.setFileTimeNow}
          onChange={(_, data) => updateOptions({ setFileTimeNow: !!data.checked })}
        />
        {o.chapters === "split" && (
          <Checkbox
            label="Force keyframes at cuts"
            checked={o.forceKeyframes}
            onChange={(_, data) => updateOptions({ forceKeyframes: !!data.checked })}
          />
        )}
      </div>

      <div className={mergeClasses(styles.inline, styles.full)}>
        <Checkbox
          label="Custom arguments"
          checked={o.customArgsEnabled}
          onChange={(_, data) => updateOptions({ customArgsEnabled: !!data.checked })}
        />
        <Input
          size="small"
          className={mergeClasses(styles.grow, styles.mono)}
          placeholder="Extra yt-dlp options, for example --proxy http://127.0.0.1:8080"
          disabled={!o.customArgsEnabled}
          value={o.customArgs}
          onChange={(_, data) => updateOptions({ customArgs: data.value })}
        />
      </div>
    </div>
  );
}
