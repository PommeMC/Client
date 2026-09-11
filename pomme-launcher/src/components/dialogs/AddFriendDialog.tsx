import { Dialog } from "radix-ui";
import { useState } from "react";
import { useAppStore } from "../../lib/store";

const ENTER_KEY = "Enter";

export type AddFriendDialogProps = {
  onSubmit: (name: string) => Promise<void>;
};

export function AddFriendDialog(dialogProps: AddFriendDialogProps) {
  const setOpenedDialog = useAppStore((state) => state.setOpenedDialog);
  const [name, setName] = useState("");
  const [loading, setLoading] = useState(false);

  const handleSubmit = async () => {
    const trimmed = name.trim();
    const isEmpty = trimmed === "";
    if (isEmpty) {
      return;
    }
    if (loading) {
      return;
    }
    setLoading(true);
    try {
      await dialogProps.onSubmit(trimmed);
      setOpenedDialog(null);
    } finally {
      setLoading(false);
    }
  };

  const submitOnEnter = (event: React.KeyboardEvent<HTMLInputElement>) => {
    const isEnter = event.key === ENTER_KEY;
    if (isEnter) {
      handleSubmit();
    }
  };

  return (
    <>
      <Dialog.Title className="dialog-title">Add Friend</Dialog.Title>

      <div className="dialog-fields">
        <div className="dialog-field">
          <label className="field-label">JAVA PROFILE NAME</label>
          <input
            className="field"
            value={name}
            onChange={(event) => setName(event.target.value)}
            onKeyDown={submitOnEnter}
            placeholder="Notch"
            autoFocus
          />
        </div>
      </div>

      <div className="dialog-actions">
        <button
          className="button-secondary"
          disabled={loading}
          onClick={() => setOpenedDialog(null)}
        >
          Cancel
        </button>
        <button className="button-primary" disabled={loading} onClick={handleSubmit}>
          {loading ? "..." : "Send Request"}
        </button>
      </div>
    </>
  );
}
