import { useState } from "react";
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
  makeStyles,
  MessageBar,
  MessageBarBody,
  ProgressBar,
  Text,
  tokens,
} from "@fluentui/react-components";
import { ArrowDownloadRegular } from "@fluentui/react-icons";
import { formatBytes } from "../format";
import { useApp } from "../store";

const useStyles = makeStyles({
  surface: { maxWidth: "500px" },
  content: { display: "flex", flexDirection: "column", gap: "14px" },
  muted: { color: tokens.colorNeutralForeground3 },
  detail: { overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", color: tokens.colorNeutralForeground3 },
  steps: { display: "flex", flexDirection: "column", gap: "6px" },
});

const PHASES = ["uv", "python", "packages", "model"];
const PHASE_LABEL: Record<string, string> = {
  uv: "Getting the installer",
  python: "Setting up a private Python",
  packages: "Installing Demucs and PyTorch",
  model: "Downloading the separation models",
};
/** What to tell the user about the step they are waiting for. */
const PHASE_HINT: Record<string, string> = {
  uv: "A small download.",
  python: "A private copy of Python that only this app uses.",
  packages: "This is the biggest step.",
  model: "The last step. The first start can take a minute or two while your computer checks the new files.",
};

/** Asks before downloading the audio-separation add-on, then shows how the install is going. */
export function StemsDialog() {
  const styles = useStyles();
  const prompt = useApp((s) => s.stemsPrompt);
  const install = useApp((s) => s.stemsInstall);
  const stems = useApp((s) => s.stems);
  const { installStems, cancelStemsInstall, dismissStemsPrompt } = useApp.getState();
  const [gpu, setGpu] = useState(false);

  const installing = install !== null && install.error === null;
  const step = install ? Math.max(0, PHASES.indexOf(install.phase)) + 1 : 0;
  // Some steps know how much there is to do; for the others the bar just moves back and forth.
  const measured = install !== null && install.total > 0;
  const nvidia = stems?.nvidiaFound ?? false;

  return (
    <Dialog open={prompt !== null} onOpenChange={(_, data) => !data.open && !installing && dismissStemsPrompt()}>
      <DialogSurface className={styles.surface}>
        <DialogBody>
          <DialogTitle>Separate audio</DialogTitle>
          <DialogContent className={styles.content}>
            {install === null && (
              <>
                <Body1>
                  Splits the sound of any audio or video file into vocals, drums, bass and more. This needs a one-time
                  download of about <b>0.8 GB</b> (the Demucs separation tool and its models). It runs on your computer,
                  and your files are never uploaded.
                </Body1>
                {nvidia && (
                  <Checkbox
                    checked={gpu}
                    onChange={(_, data) => setGpu(!!data.checked)}
                    label="Use my NVIDIA graphics card (faster, but adds about 4 GB)"
                  />
                )}
                <Caption1 className={styles.muted}>You can remove it again any time in Settings.</Caption1>
              </>
            )}

            {installing && (
              <div className={styles.steps}>
                <Text weight="semibold">
                  Step {step} of {PHASES.length}: {PHASE_LABEL[install.phase] ?? "Working"}
                </Text>
                <ProgressBar value={measured ? Math.min(0.99, install.received / install.total) : undefined} />
                <Caption1 className={styles.detail} title={install.detail}>
                  {measured
                    ? `${formatBytes(install.received)} of ${install.phase === "packages" ? "about " : ""}${formatBytes(install.total)}`
                    : install.phase === "model"
                      ? PHASE_LABEL.model
                      : install.detail || "Working…"}
                </Caption1>
                <Caption1 className={styles.muted}>{PHASE_HINT[install.phase]}</Caption1>
                <Caption1 className={styles.muted}>This can take a few minutes. You can keep using the app.</Caption1>
              </div>
            )}

            {install?.error && (
              <MessageBar intent="error" layout="multiline">
                <MessageBarBody>{install.error}</MessageBarBody>
              </MessageBar>
            )}
          </DialogContent>
          <DialogActions>
            {installing ? (
              <Button onClick={cancelStemsInstall}>Cancel install</Button>
            ) : (
              <>
                <Button appearance="secondary" onClick={dismissStemsPrompt}>
                  {install?.error ? "Close" : "Not now"}
                </Button>
                <Button appearance="primary" icon={<ArrowDownloadRegular />} onClick={() => installStems(gpu && nvidia)}>
                  {install?.error ? "Try again" : "Install"}
                </Button>
              </>
            )}
          </DialogActions>
        </DialogBody>
      </DialogSurface>
    </Dialog>
  );
}
