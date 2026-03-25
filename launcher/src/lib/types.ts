export type Page = "home" | "installations" | "servers" | "friends" | "mods" | "news" | "settings";

export interface AuthAccount {
  username: string;
  uuid: string;
  access_token: string;
  expires_at: number;
}

export interface Installation {
  id: string;
  name: string;
  version: string;
  lastPlayed: string;
  directory: string;
  width: number;
  height: number;
}

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
