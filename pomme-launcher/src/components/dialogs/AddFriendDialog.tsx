import { useState } from "react";
import { useAppStateContext } from "../../lib/state";

export type AddFriendDialogProps = {
  onSubmit: (name: string) => Promise<void>;
};

export function AddFriendDialog(dialogProps: AddFriendDialogProps) {
  const { setOpenedDialog } = useAppStateContext();
  const [name, setName] = useState("");
  const [loading, setLoading] = useState(false);

  const handleSubmit = async () => {
    const trimmed = name.trim();
    if (!trimmed || loading) return;
    setLoading(true);
    try {
      await dialogProps.onSubmit(trimmed);
      setOpenedDialog(null);
    } finally {
      setLoading(false);
    }
  };

  return (
    <div className="dialog" onClick={(e) => e.stopPropagation()}>
      <h2 className="dialog-title">Add Friend</h2>

      <div className="dialog-fields">
        <div className="dialog-field">
          <label>JAVA PROFILE NAME</label>
          <input
            value={name}
            onChange={(e) => setName(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && handleSubmit()}
            placeholder="Notch"
            autoFocus
          />
        </div>
      </div>

      <div className="dialog-actions">
        <button className="dialog-cancel" disabled={loading} onClick={() => setOpenedDialog(null)}>
          Cancel
        </button>
        <button className="dialog-save" disabled={loading} onClick={handleSubmit}>
          {loading ? "..." : "Send Request"}
        </button>
      </div>
    </div>
  );
}
