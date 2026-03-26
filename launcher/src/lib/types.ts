import { InstallationDialogProps } from "../components/dialogs/InstallationDialog.tsx";
import { ConfirmDialogProps } from "../components/dialogs/ConfirmDialog.tsx";

export type Page = "home" | "installations" | "servers" | "friends" | "mods" | "news" | "settings";

// dialog_name: typeof props
type DialogMap = {
  installation: InstallationDialogProps;
  confirm_dialog: ConfirmDialogProps;
};

export type OpenedDialog =
  | {
      [K in keyof DialogMap]: DialogMap[K] extends undefined
        ? { name: K }
        : { name: K; props: DialogMap[K] };
    }[keyof DialogMap]
  | null;

export interface AuthAccount {
  username: string;
  uuid: string;
  access_token: string;
  expires_at: number;
}

export interface Installation {
  id: string;
  icon: string | null;
  name: string;
  version: string;
  lastPlayed: number | null;
  createdAt: number;
  directory: string;
  width: number;
  height: number;
  canDelete: boolean;
}

export type InstallationError =
  | { kind: "InvalidName" }
  | { kind: "NameTooLong"; detail: number }
  | { kind: "InvalidDirectory" }
  | { kind: "DirectoryTooLong"; detail: number }
  | { kind: "InvalidCharacter"; detail: string }
  | { kind: "TrailingDot" }
  | { kind: "DirectoryAlreadyExists" }
  | { kind: "Io"; detail: string }
  | { kind: "Json"; detail: string };

export interface GameVersion {
  id: string;
  version_type: string;
}

export interface PatchNote {
  title: string;
  version: string;
  date: string;
  summary: string;
  image_url: string;
  entry_type: string;
  content_path: string;
}

export interface DownloadProgress {
  downloaded: number;
  total: number;
  status: string;
}

export interface LauncherSettings {
  language: string;
  keepLauncherOpen: boolean;
  launchWithConsole: boolean;
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

export interface ServerStatus {
  online: boolean;
  players: number;
  max_players: number;
  ping_ms: number;
  motd: string;
  version: string;
}
