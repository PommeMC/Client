import { AlertDialogProps } from "../components/dialogs/AlertDialog.tsx";
import { ConfirmDialogProps } from "../components/dialogs/ConfirmDialog.tsx";
import { InstallationDialogProps } from "../components/dialogs/InstallationDialog.tsx";

export type Page = "home" | "installations" | "servers" | "friends" | "mods" | "news" | "settings";

export type LaunchingStatus = null | "checking_assets" | "launching" | "installing";

// dialog_name: typeof props
type DialogMap = {
  installation: InstallationDialogProps;
  confirm_dialog: ConfirmDialogProps;
  alert_dialog: AlertDialogProps;
};

export type OpenedDialog =
  | {
      [K in keyof DialogMap]: DialogMap[K] extends undefined
        ? { name: K }
        : { name: K; props: DialogMap[K] };
    }[keyof DialogMap]
  | null;

export interface DownloadProgress {
  downloaded: number;
  total: number;
  status: string;
}

export interface Server {
  id: string;
  name: string;
  ip: string;
  category: string;
  players: number;
  max_players: number;
  ping: number;
  online: boolean;
  motd: string;
  version: string;
}
