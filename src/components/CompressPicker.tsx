import { useEffect, useState } from "react";
import {
  Body1,
  Button,
  Caption1,
  Checkbox,
  Dialog,
  DialogActions,
  DialogBody,
  DialogContent,
  DialogSurface,
  DialogTitle,
  Input,
  makeStyles,
  MessageBar,
  MessageBarBody,
  Radio,
  RadioGroup,
  Spinner,
  tokens,
} from "@fluentui/react-components";
import { api, type CompressChoice, type CompressPlan } from "../api";
import { megabytes } from "../format";
import { useApp } from "../store";

const useStyles = makeStyles({
  surface: { maxWidth: "500px" },
  content: { display: "flex", flexDirection: "column", gap: "12px" },
  option: { display: "flex", flexDirection: "column" },
  hint: { color: tokens.colorNeutralForeground3, paddingLeft: "32px" },
  sizeRow: { display: "flex", alignItems: "center", gap: "8px" },
  sizeInput: { width: "88px" },
  presets: { display: "flex", flexWrap: "wrap", gap: "6px", paddingLeft: "32px" },
  result: { display: "flex", alignItems: "center", gap: "8px", minHeight: "24px" },
  muted: { color: tokens.colorNeutralForeground3 },
});

/** Sizes people often have to fit: Discord, and other common upload limits. */
const PRESETS = [10, 25, 50, 100];

type Mode = "same" | "half" | "size";

/** Asks how small to make a file. */
export function CompressPicker() {
  const picker = useApp((s) => s.compressPicker);
  // A fresh dialog each time, so it always starts from the right choice.
  return picker ? <PickerDialog key={`${picker.id}${picker.path}`} /> : null;
}

function PickerDialog() {
  const styles = useStyles();
  const picker = useApp((s) => s.compressPicker)!;
  const { confirmCompress, dismissCompressPicker } = useApp.getState();
  const initial = picker.initial.goal;
  const [mode, setMode] = useState<Mode>(initial.mode === "same" ? "same" : initial.half ? "half" : "size");
  const [mb, setMb] = useState(initial.mode === "size" && !initial.half ? String(initial.bytes / 1_000_000) : "10");
  const [newer, setNewer] = useState(picker.initial.newerFormat);
  const [plan, setPlan] = useState<CompressPlan | null>(null);
  const [original, setOriginal] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);

  const typed = Number(mb.replace(",", "."));
  const bytes =
    mode === "half"
      ? original
        ? Math.floor(original / 2)
        : null
      : mode === "size" && isFinite(typed) && typed > 0
        ? Math.round(typed * 1_000_000)
        : null;
  const choice: CompressChoice | null =
    mode === "same"
      ? { goal: { mode: "same" }, newerFormat: newer }
      : bytes
        ? { goal: { mode: "size", bytes, ...(mode === "half" ? { half: true } : {}) }, newerFormat: newer }
        : null;

  // Works out what the choice would do, a moment after it stops changing.
  const key = JSON.stringify(choice);
  useEffect(() => {
    let current = true;
    const asked: CompressChoice = choice ?? { goal: { mode: "same" }, newerFormat: newer };
    const timer = setTimeout(() => {
      api
        .compressPlan(picker.source, asked, picker.format)
        .then((p) => {
          if (!current) return;
          if (p.size > 0) setOriginal(p.size);
          setPlan(choice ? p : null);
          setError(null);
        })
        .catch((e) => current && setError(String(e)));
    }, 200);
    return () => {
      current = false;
      clearTimeout(timer);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key, picker.source, picker.format]);

  const video = picker.hasVideo;
  const problem = error ?? plan?.problem ?? null;
  const ready = choice !== null && plan !== null && !problem && JSON.stringify(choice) === key;

  let result: string | null = null;
  if (plan && !plan.problem && choice) {
    const picture = video && plan.height ? `${plan.height}p${plan.fps ? ` at ${Math.round(plan.fps)} fps` : ""}` : null;
    if (choice.goal.mode === "same") {
      result = video ? `Keeps ${picture ?? "the picture"}. How much smaller depends on how the video was made.` : `About ${megabytes(plan.estimate ?? 0)}`;
    } else {
      result = [`About ${megabytes(plan.estimate ?? 0)}`, picture].filter(Boolean).join(" · ");
    }
  }

  return (
    <Dialog open onOpenChange={(_, data) => !data.open && dismissCompressPicker()}>
      <DialogSurface className={styles.surface}>
        <DialogBody>
          <DialogTitle>Compress</DialogTitle>
          <DialogContent className={styles.content}>
            <Body1>Make the {video ? "video" : "file"} smaller.</Body1>
            <RadioGroup value={mode} onChange={(_, data) => setMode(data.value as Mode)}>
              <div className={styles.option}>
                <Radio value="same" label={video ? "Same look, smaller file" : "Same sound, smaller file"} />
                <Caption1 className={styles.hint}>
                  {video
                    ? "Best for keeping. Phone and screen recordings usually get several times smaller."
                    : "Saved at a good everyday quality."}
                </Caption1>
              </div>
              <div className={styles.option}>
                <Radio
                  value="half"
                  label={original ? `Half the size (about ${megabytes(original / 2)})` : "Half the size"}
                  disabled={original === null}
                />
              </div>
              <div className={styles.option}>
                <div className={styles.sizeRow}>
                  <Radio value="size" label="Up to" />
                  <Input
                    className={styles.sizeInput}
                    size="small"
                    inputMode="decimal"
                    value={mb}
                    aria-label="Size in megabytes"
                    onChange={(_, data) => {
                      setMb(data.value);
                      setMode("size");
                    }}
                    contentAfter={<span className={styles.muted}>MB</span>}
                  />
                </div>
                <div className={styles.presets}>
                  {PRESETS.map((n) => (
                    <Button
                      key={n}
                      size="small"
                      appearance={mode === "size" && typed === n ? "primary" : "secondary"}
                      disabled={original !== null && n * 1_000_000 >= original}
                      onClick={() => {
                        setMb(String(n));
                        setMode("size");
                      }}
                    >
                      {n} MB
                    </Button>
                  ))}
                </div>
                <Caption1 className={styles.hint}>10 MB fits Discord without Nitro.</Caption1>
              </div>
            </RadioGroup>

            {video && (
              <div className={styles.option}>
                <Checkbox checked={newer} onChange={(_, data) => setNewer(!!data.checked)} label="Even smaller, with a newer format" />
                <Caption1 className={styles.hint}>
                  Takes longer to make, and some older phones and browsers can't play it.
                </Caption1>
              </div>
            )}

            {problem ? (
              <MessageBar intent="warning">
                <MessageBarBody>{problem}</MessageBarBody>
              </MessageBar>
            ) : plan?.alreadySmall ? (
              <MessageBar intent="info">
                <MessageBarBody>
                  This {video ? "video" : "file"} is already stored compactly, so it won't get much smaller this way. Choose a
                  size instead.
                </MessageBarBody>
              </MessageBar>
            ) : (
              <div className={styles.result}>
                {result ? <Body1>{result}</Body1> : choice ? <Spinner size="extra-tiny" /> : null}
              </div>
            )}
          </DialogContent>
          <DialogActions>
            <Button appearance="secondary" onClick={dismissCompressPicker}>
              Cancel
            </Button>
            <Button appearance="primary" disabled={!ready} onClick={() => choice && confirmCompress(choice, plan?.height ?? null)}>
              {picker.id ? "OK" : "Add to list"}
            </Button>
          </DialogActions>
        </DialogBody>
      </DialogSurface>
    </Dialog>
  );
}
