import { createContext, createElement, ReactNode, useContext, useEffect, useState } from "react";
import { commands } from "../bindings";
import { AuthAccount } from "../bindings/pomme_launcher/auth";
import { GameVersion, PatchNote } from "../bindings/pomme_launcher/commands";
import { LauncherSettings } from "../bindings/pomme_launcher/settings";
import { useDropdown } from "./hooks";
import { useInstallations } from "./installations";
import { useServers } from "./servers";
import { DownloadProgress, LaunchingStatus, OpenedDialog, Page } from "./types";

const useLauncherSettings = () => {
  const [launcherSettings, setLauncherSettings] = useState<LauncherSettings>({
    language: "English",
    keepLauncherOpen: true,
    launchWithConsole: false,
  });

  useEffect(() => {
    commands
      .loadLauncherSettings()
      .then((settings) => setLauncherSettings(settings))
      .catch(console.error);
  }, []);

  const setLanguage = async (language: string) => {
    let res = await commands.setLauncherLanguage(language);
    if (res.ok) {
      setLauncherSettings((prev) => ({ ...prev, language }));
    } else {
      console.error("Error while setting `launcherLanguage: ", res.error);
    }
  };
  const setKeepLauncherOpen = async (keep: boolean) => {
    let res = await commands.setKeepLauncherOpen(keep);
    if (res.ok) {
      setLauncherSettings((prev) => ({ ...prev, keepLauncherOpen: keep }));
    } else {
      console.error("Error while setting `keepLauncherOpen`: ", res.error);
    }
  };
  const setLaunchWithConsole = async (launch: boolean) => {
    let res = await commands.setLaunchWithConsole(launch);
    if (res.ok) {
      setLauncherSettings((prev) => ({ ...prev, launchWithConsole: launch }));
    } else {
      console.error("Error while setting `launchWithConsole`: ", res.error);
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
