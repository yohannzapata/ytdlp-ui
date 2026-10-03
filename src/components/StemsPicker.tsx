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
  Divider,
  makeStyles,
  tokens,
} from "@fluentui/react-components";
import { STEM_PARTS, leftoverParts, restName, type StemsChoice } from "../format";
import { useApp } from "../store";

const useStyles = makeStyles({
  surface: { maxWidth: "460px" },
  content: { display: "flex", flexDirection: "column", gap: "10px" },
  list: { display: "flex", flexDirection: "column" },
  item: { display: "flex", alignItems: "center", justifyContent: "space-between", gap: "12px", minHeight: "34px" },
  muted: { color: tokens.colorNeutralForeground3 },
  rest: { display: "flex", flexDirection: "column" },
  restHint: { color: tokens.colorNeutralForeground3, paddingLeft: "32px" },
});

/** Asks which parts of a file's sound to save as files of their own. */
export function StemsPicker() {
  const picker = useApp((s) => s.stemsPicker);
  // A fresh dialog each time, so it always starts from the right choice.
  return picker ? <PickerDialog key={`${picker.id}${picker.path}`} initial={picker.initial} addsRow={picker.id === null} /> : null;
}

function PickerDialog({ initial, addsRow }: { initial: StemsChoice; addsRow: boolean }) {
  const styles = useStyles();
  const { confirmStems, dismissStemsPicker } = useApp.getState();
  const [parts, setParts] = useState(initial.parts);
  const [rest, setRest] = useState(initial.rest);

  const left = leftoverParts(parts);
  const toggle = (part: string, on: boolean) =>
    setParts((now) => (on ? [...now, part] : now.filter((p) => p !== part)));

  return (
    <Dialog open onOpenChange={(_, data) => !data.open && dismissStemsPicker()}>
      <DialogSurface className={styles.surface}>
        <DialogBody>
          <DialogTitle>Separate audio</DialogTitle>
          <DialogContent className={styles.content}>
            <Body1>Choose the parts to save as separate files.</Body1>
            <div className={styles.list}>
              {STEM_PARTS.map((part) => (
                <div key={part.value} className={styles.item}>
                  <Checkbox
                    checked={parts.includes(part.value)}
                    onChange={(_, data) => toggle(part.value, !!data.checked)}
                    label={part.label}
                  />
                  {part.hint && <Caption1 className={styles.muted}>{part.hint}</Caption1>}
                </div>
              ))}
            </div>
            {parts.length > 0 && left.length > 0 && (
              <>
                <Divider />
                <div className={styles.rest}>
                  <Checkbox
                    checked={rest}
                    onChange={(_, data) => setRest(!!data.checked)}
                    label={`Also save the rest as one file (“${restName(parts)}”)`}
                  />
                  <Caption1 className={styles.restHint}>Everything you didn't choose, mixed back together.</Caption1>
                </div>
              </>
            )}
          </DialogContent>
          <DialogActions>
            <Button appearance="secondary" onClick={dismissStemsPicker}>
              Cancel
            </Button>
            <Button appearance="primary" disabled={parts.length === 0} onClick={() => confirmStems({ parts, rest })}>
              {addsRow ? "Add to list" : "OK"}
            </Button>
          </DialogActions>
        </DialogBody>
      </DialogSurface>
    </Dialog>
  );
}
