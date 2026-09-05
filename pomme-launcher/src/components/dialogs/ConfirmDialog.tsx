import { Dialog } from "radix-ui";
import { useState } from "react";
import { useAppStore } from "../../lib/store";

export type ConfirmDialogProps = {
  title: string;
  message: string;
  onCancel?: () => void | Promise<void>;
  onConfirm?: () => void | Promise<void>;
};

export function ConfirmDialog(dialogProps: ConfirmDialogProps) {
  const setOpenedDialog = useAppStore((state) => state.setOpenedDialog);
  const [loading, setLoading] = useState(false);

  const cancel = async () => {
    if (loading) {
      return;
    }
    setOpenedDialog(null);
    try {
      await dialogProps.onCancel?.();
    } catch (error) {
      console.error(error);
    }
  };

  const confirm = async () => {
    if (loading) {
      return;
    }
    setLoading(true);
    try {
      await dialogProps.onConfirm?.();
      setOpenedDialog(null);
    } catch (error) {
      console.error(error);
    } finally {
      setLoading(false);
    }
  };

  return (
    <>
      <Dialog.Title className="dialog-title">{dialogProps.title}</Dialog.Title>

      <div className="dialog-fields">
        <p className="dialog-text">{dialogProps.message}</p>
      </div>

      <div className="dialog-actions">
        <button className="button-secondary" disabled={loading} onClick={cancel}>
          Cancel
        </button>
        <button className="button-primary" disabled={loading} onClick={confirm}>
          {loading ? "..." : "Confirm"}
        </button>
      </div>
    </>
  );
}
