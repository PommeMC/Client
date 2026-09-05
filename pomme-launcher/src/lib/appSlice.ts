import { StateCreator } from "zustand";
import { commands } from "../bindings";
import { AuthAccount } from "../bindings/pomme_launcher/auth";
import { GameVersion, PatchNote } from "../bindings/pomme_launcher/commands";
import { Installation } from "../bindings/pomme_launcher/installations";
import type { AppStore } from "./store";
import { DownloadProgress, LaunchingStatus, OpenedDialog, Page } from "./types";

export const LATEST_RELEASE_ID = "latest-release";
export const LATEST_SNAPSHOT_ID = "latest-snapshot";
export const MOD_FILTER_ALL = "all";
const SIDEBAR_COLLAPSED_KEY = "sidebarCollapsed";

export type SelectedNote = {
  title: string;
  body: string;
  image_url: string;
  entry_type: string;
  date: string;
};

export type ModView = "list" | "grid";

export type AppSlice = {
  page: Page;
  setPage: (page: Page) => void;
  openedDialog: OpenedDialog;
  setOpenedDialog: (openedDialog: OpenedDialog) => void;
  accountMenuOpen: boolean;
  setAccountMenuOpen: (accountMenuOpen: boolean) => void;
  sidebarCollapsed: boolean;
  setSidebarCollapsed: (sidebarCollapsed: boolean) => void;
  accounts: AuthAccount[];
  setAccounts: (accounts: AuthAccount[]) => void;
  upsertAccount: (account: AuthAccount) => void;
  removeAccountFromList: (uuid: string) => void;
  activeIndex: number;
  setActiveIndex: (activeIndex: number) => void;
  modView: ModView;
  setModView: (modView: ModView) => void;
  modSearch: string;
  setModSearch: (modSearch: string) => void;
  modFilter: string;
  setModFilter: (modFilter: string) => void;
  versions: GameVersion[];
  setVersions: (versions: GameVersion[]) => void;
  launchingStatus: LaunchingStatus;
  setLaunchingStatus: (launchingStatus: LaunchingStatus) => void;
  authLoading: boolean;
  setAuthLoading: (authLoading: boolean) => void;
  authUrl: string | null;
  setAuthUrl: (authUrl: string | null) => void;
  status: string;
  setStatus: (status: string) => void;
  news: PatchNote[];
  setNews: (news: PatchNote[]) => void;
  skinUrl: string | null;
  setSkinUrl: (skinUrl: string | null) => void;
  downloadProgress: DownloadProgress | null;
  setDownloadProgress: (downloadProgress: DownloadProgress | null) => void;
  downloadedVersions: Set<string>;
  addDownloadedVersions: (versions: string[]) => void;
  selectedNote: SelectedNote | null;
  setSelectedNote: (selectedNote: SelectedNote | null) => void;
  installations: Installation[];
  loadInstallations: () => Promise<void>;
  addInstallation: (installation: Installation) => void;
  replaceInstallation: (installation: Installation) => void;
  removeInstallation: (id: string) => void;
  activeInstall: Installation | null;
  setActiveInstall: (activeInstall: Installation | null) => void;
};

function readSidebarCollapsed(): boolean {
  const stored = localStorage.getItem(SIDEBAR_COLLAPSED_KEY);
  const collapsed = stored === "true";
  return collapsed;
}

type AccountState = Pick<AppSlice, "accounts" | "activeIndex">;

export function getAccount(state: AccountState): AuthAccount | null {
  const account = state.accounts[state.activeIndex] ?? null;
  return account;
}

export function getAccountUuid(state: AccountState): string | null {
  const uuid = getAccount(state)?.uuid ?? null;
  return uuid;
}

export function isBuiltInInstallation(installation: Installation): boolean {
  const isLatestRelease = installation.id === LATEST_RELEASE_ID;
  const isLatestSnapshot = installation.id === LATEST_SNAPSHOT_ID;
  const isBuiltIn = isLatestRelease || isLatestSnapshot;
  return isBuiltIn;
}

function getLatestRelease(installations: Installation[]): Installation | null {
  const latestRelease =
    installations.find((installation) => installation.id === LATEST_RELEASE_ID) ?? null;
  return latestRelease;
}

function getNextActiveInstall(
  remaining: Installation[],
  removedIndex: number,
): Installation | null {
  const neighbour = remaining[removedIndex] ?? remaining[removedIndex - 1] ?? null;
  const hasNeighbour = neighbour !== null;
  const onlyBuiltInLeft = remaining.every(isBuiltInInstallation);

  let next = neighbour;
  if (!hasNeighbour) {
    next = getLatestRelease(remaining) ?? neighbour;
  }
  if (onlyBuiltInLeft) {
    next = getLatestRelease(remaining) ?? neighbour;
  }
  return next;
}

export const createAppSlice: StateCreator<AppStore, [], [], AppSlice> = (set, get) => ({
  page: "home",
  setPage: (page) => set({ page }),

  openedDialog: null,
  setOpenedDialog: (openedDialog) => set({ openedDialog }),

  accountMenuOpen: false,
  setAccountMenuOpen: (accountMenuOpen) => set({ accountMenuOpen }),

  sidebarCollapsed: readSidebarCollapsed(),
  setSidebarCollapsed: (sidebarCollapsed) => {
    localStorage.setItem(SIDEBAR_COLLAPSED_KEY, String(sidebarCollapsed));
    set({ sidebarCollapsed });
  },

  accounts: [],
  setAccounts: (accounts) => set({ accounts }),
  upsertAccount: (account) => {
    const otherAccounts = get().accounts.filter((existing) => existing.uuid !== account.uuid);
    set({ accounts: [...otherAccounts, account], activeIndex: otherAccounts.length });
  },
  removeAccountFromList: (uuid) => {
    const remaining = get().accounts.filter((account) => account.uuid !== uuid);
    set({ accounts: remaining, activeIndex: 0 });
  },
  activeIndex: 0,
  setActiveIndex: (activeIndex) => set({ activeIndex }),

  modView: "list",
  setModView: (modView) => set({ modView }),
  modSearch: "",
  setModSearch: (modSearch) => set({ modSearch }),
  modFilter: MOD_FILTER_ALL,
  setModFilter: (modFilter) => set({ modFilter }),

  versions: [],
  setVersions: (versions) => set({ versions }),
  launchingStatus: null,
  setLaunchingStatus: (launchingStatus) => set({ launchingStatus }),
  authLoading: false,
  setAuthLoading: (authLoading) => set({ authLoading }),
  authUrl: null,
  setAuthUrl: (authUrl) => set({ authUrl }),
  status: "",
  setStatus: (status) => set({ status }),
  news: [],
  setNews: (news) => set({ news }),
  skinUrl: null,
  setSkinUrl: (skinUrl) => set({ skinUrl }),
  downloadProgress: null,
  setDownloadProgress: (downloadProgress) => set({ downloadProgress }),
  downloadedVersions: new Set(),
  addDownloadedVersions: (versions) => {
    const downloadedVersions = new Set([...get().downloadedVersions, ...versions]);
    set({ downloadedVersions });
  },
  selectedNote: null,
  setSelectedNote: (selectedNote) => set({ selectedNote }),

  installations: [],
  loadInstallations: async () => {
    const result = await commands.loadInstallations();
    const succeeded = result.ok;
    if (!succeeded) {
      set({ status: `Failed to load installations: ${result.error.kind}` });
      return;
    }
    const installations = result.value;
    const activeInstall = get().activeInstall ?? installations[0] ?? null;
    set({ installations, activeInstall });
  },
  addInstallation: (installation) => {
    const installations = [...get().installations, installation];
    set({ installations, activeInstall: installation });
  },
  replaceInstallation: (installation) => {
    const installations = get().installations.map((existing) => {
      const isSame = existing.id === installation.id;
      const next = isSame ? installation : existing;
      return next;
    });
    set({ installations, activeInstall: installation });
  },
  removeInstallation: (id) => {
    const { installations, activeInstall } = get();
    const removedIndex = installations.findIndex((installation) => installation.id === id);
    const remaining = installations.filter((installation) => installation.id !== id);
    const activeWasRemoved = activeInstall?.id === id;
    if (!activeWasRemoved) {
      set({ installations: remaining });
      return;
    }
    const nextActive = getNextActiveInstall(remaining, removedIndex);
    set({ installations: remaining, activeInstall: nextActive });
  },
  activeInstall: null,
  setActiveInstall: (activeInstall) => set({ activeInstall }),
});
