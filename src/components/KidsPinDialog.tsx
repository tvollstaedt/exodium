import { unlockKids } from "../stores/kids";
import { PinDialog, pinAnswer } from "./PinDialog";

const FIELDS = [{ testId: "kids-pin-input", placeholder: "PIN" }];

/** The kids screen's one door (§26). */
export function KidsPinDialog(props: { open: boolean; onClose: () => void; onUnlocked: () => void }) {
  const submit = async ([pin]: string[]) => {
    const check = await unlockKids(pin);
    if (check.ok) { props.onUnlocked(); }
    return pinAnswer(check);
  };

  return (
    <PinDialog
      open={props.open}
      title="Unlock Exodium"
      fields={FIELDS}
      submitLabel="Unlock"
      autoSubmit
      testId="kids-pin-dialog"
      submitTestId="kids-pin-submit"
      onSubmit={submit}
      onClose={props.onClose}
    />
  );
}
