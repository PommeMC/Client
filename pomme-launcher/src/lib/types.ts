import { Installation } from "../bindings/pomme_launcher/installations";
import { AlertDialogProps } from "../components/dialogs/AlertDialog";
import { ConfirmDialogProps } from "../components/dialogs/ConfirmDialog";
import { InstallationDialogProps } from "../components/dialogs/InstallationDialog";
import { ServerDialogProps } from "../components/dialogs/ServerDialog";

export type Page = "home" | "installations" | "servers" | "friends" | "mods" | "news" | "settings";

export type LaunchingStatus = null | "checking_assets" | "launching" | "installing";

// dialog_name: typeof props
type DialogMap = {
  installation_dialog: InstallationDialogProps;
  server_dialog: ServerDialogProps;
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

export type handleLaunchType = (options?: {
  serverIp?: string;
  serverVersion?: string;
  install?: Installation;
}) => Promise<void>;
