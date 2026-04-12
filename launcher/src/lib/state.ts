import { invoke } from "@tauri-apps/api/core";
import { createContext, createElement, ReactNode, useContext, useEffect, useState } from "react";
import { useDropdown } from "./hooks";
import { useInstallations } from "./installations.ts";
import { useServers } from "./servers";
import {
  AuthAccount,
  DownloadProgress,
  GameVersion,
  LauncherSettings,
  LaunchingStatus,
  OpenedDialog,
  Page,
  PatchNote,
} from "./types";

const useLauncherSettings = () => {
  const [launcherSettings, setLauncherSettings] = useState<LauncherSettings>({
    language: "English",
    keepLauncherOpen: true,
    launchWithConsole: false,
  });

  useEffect(() => {
    invoke<LauncherSettings>("load_launcher_settings")
      .then((settings) => setLauncherSettings(settings))
      .catch(console.error);
  }, []);

  const setLanguage = async (language: string) => {
    try {
      await invoke("set_launcher_language", { language });
      setLauncherSettings((prev) => ({ ...prev, language }));
    } catch (err) {
      console.error(err);
    }
  };
  const setKeepLauncherOpen = async (keep: boolean) => {
    try {
      await invoke("set_keep_launcher_open", { keep });
      setLauncherSettings((prev) => ({ ...prev, keepLauncherOpen: keep }));
    } catch (err) {
      console.error(err);
    }
  };
  const setLaunchWithConsole = async (launch: boolean) => {
    try {
      await invoke("set_launch_with_console", { launch });
      setLauncherSettings((prev) => ({ ...prev, launchWithConsole: launch }));
    } catch (err) {
      console.error(err);
    }
  };

  return {
    ...launcherSettings,
    setLanguage,
    setKeepLauncherOpen,
    setLaunchWithConsole,
  };
};

const useAppState = () => {
  const [page, setPage] = useState<Page>("home");
  const [openedDialog, setOpenedDialog] = useState<OpenedDialog>(null);
  const [accounts, setAccounts] = useState<AuthAccount[]>([]);
  const [activeIndex, setActiveIndex] = useState(0);
  const accountDropdown = useDropdown();

  const [server, setServer] = useState("");

  const [modView, setModView] = useState<"list" | "grid">("list");
  const [modSearch, setModSearch] = useState("");
  const [modFilter, setModFilter] = useState("all");
  const [versions, setVersions] = useState<GameVersion[]>([]);
  const [launchingStatus, setLaunchingStatus] = useState<LaunchingStatus>(null);
  const [authLoading, setAuthLoading] = useState(false);
  const [status, setStatus] = useState("");
  const [news, setNews] = useState<PatchNote[]>([]);
  const [skinUrl, setSkinUrl] = useState<string | null>(null);
  const [downloadProgress, setDownloadProgress] = useState<DownloadProgress | null>(null);
  const [downloadedVersions, setDownloadedVersions] = useState<Set<string>>(new Set());

  const account = accounts[activeIndex] || null;
  const username = account?.username || "Steve";
  const [selectedNote, setSelectedNote] = useState<{
    title: string;
    body: string;
    image_url: string;
    entry_type: string;
    date: string;
  } | null>(null);

  return {
    account,
    accountDropdown,
    page,
    setPage,
    accounts,
    setAccounts,
    activeIndex,
    setActiveIndex,
    server,
    setServer,
    modView,
    setModView,
    modSearch,
    setModSearch,
    modFilter,
    setModFilter,
    versions,
    setVersions,
    launchingStatus,
    setLaunchingStatus,
    authLoading,
    setAuthLoading,
    status,
    setStatus,
    news,
    setNews,
    skinUrl,
    setSkinUrl,
    downloadProgress,
    setDownloadProgress,
    selectedNote,
    setSelectedNote,
    username,
    openedDialog,
    setOpenedDialog,
    downloadedVersions,
    setDownloadedVersions,

    launcherSettings: useLauncherSettings(),
    ...useServers(),
    ...useInstallations(),
  };
};

type AppState = ReturnType<typeof useAppState>;

const AppStateContext = createContext<AppState | null>(null);

export function AppStateProvider({ children }: { children: ReactNode }) {
  const state = useAppState();
  return createElement(AppStateContext.Provider, { value: state }, children);
}

export function useAppStateContext(): AppState {
  const ctx = useContext(AppStateContext);
  if (!ctx) {
    throw new Error("useAppStateContext must be used within an AppStateProvider");
  }
  return ctx;
}
