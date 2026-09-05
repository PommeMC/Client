import { Dialog } from "radix-ui";
import { useState } from "react";
import { useAppStore } from "../../lib/store";

export type AlertDialogProps = {
  title: string;
  message: string;
  onClose?: () => void | Promise<void>;
};

export default function AlertDialog(dialogProps: AlertDialogProps) {
  const setOpenedDialog = useAppStore((state) => state.setOpenedDialog);
  const [loading, setLoading] = useState(false);

  const close = async () => {
    if (loading) {
      return;
    }
    setLoading(true);
    try {
      await dialogProps.onClose?.();
    } catch (error) {
      console.error(error);
    } finally {
      setLoading(false);
      setOpenedDialog(null);
    }
  };

  return (
    <>
      <Dialog.Title className="dialog-title">{dialogProps.title}</Dialog.Title>
      <div className="dialog-fields">
        <p className="dialog-text">{dialogProps.message}</p>
      </div>
      <div className="dialog-actions">
        <button className="button-primary" disabled={loading} onClick={close}>
          {loading ? "..." : "OK"}
        </button>
      </div>
    </>
  );
}
