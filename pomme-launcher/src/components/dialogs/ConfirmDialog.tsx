import { useState } from "react";
import { useAppStateContext } from "../../lib/state";

export type ConfirmDialogProps = {
  title: string;
  message: string;
  onCancel?: () => void | Promise<void>;
  onConfirm?: () => void | Promise<void>;
};

export function ConfirmDialog(dialogProps: ConfirmDialogProps) {
  const { setOpenedDialog } = useAppStateContext();
  const [loading, setLoading] = useState(false);

  return (
    <div className="dialog" onClick={(e) => e.stopPropagation()}>
      <h2 className="dialog-title">{dialogProps.title}</h2>

      <div className="dialog-fields">
        <p className="dialog-text">{dialogProps.message}</p>
      </div>

      <div className="dialog-actions">
        <button
          className="dialog-cancel"
          disabled={loading}
          onClick={async () => {
            if (loading) return;
            setOpenedDialog(null);
            try {
              await dialogProps.onCancel?.();
            } catch (e) {
              console.error(e);
            }
          }}
        >
          Cancel
        </button>

        <button
          className="dialog-confirm"
          disabled={loading}
          onClick={async () => {
            if (loading) return;
            setLoading(true);
            try {
              await dialogProps.onConfirm?.();
              setOpenedDialog(null);
            } catch (e) {
              console.error(e);
            } finally {
              setLoading(false);
            }
          }}
        >
          {loading ? "..." : "Confirm"}
        </button>
      </div>
    </div>
  );
}
