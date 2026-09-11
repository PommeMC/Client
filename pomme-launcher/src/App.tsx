import { getCurrentWindow } from "@tauri-apps/api/window";
import { Dialog } from "radix-ui";
import { useCallback, useEffect, useRef } from "react";

import { commands, events } from "./bindings";
import { PatchNote } from "./bindings/pomme_launcher/commands";
import { Installation } from "./bindings/pomme_launcher/installations";
import { isBuiltInInstallation } from "./lib/appSlice";
import { Activity, ACTIVITY_IDLE } from "./lib/friends";
import { getAccount, useAppStore } from "./lib/store";
import { useFriendsSync, useServerPolling } from "./lib/sync";
import { handleLaunchType } from "./lib/types";

import Aurora from "./components/Aurora";
import Navbar from "./components/Navbar";
import Titlebar from "./components/Titlebar";
import { AddFriendDialog } from "./components/dialogs/AddFriendDialog";
import AlertDialog from "./components/dialogs/AlertDialog";
import { ConfirmDialog } from "./components/dialogs/ConfirmDialog";
import { FriendSettingsDialog } from "./components/dialogs/FriendSettingsDialog";
import { InstallationDialog } from "./components/dialogs/InstallationDialog";
import { ServerDialog } from "./components/dialogs/ServerDialog";

import FriendsPage from "./pages/Friends";
import Homepage from "./pages/Home";
import InstallationsPage from "./pages/Installations";
import ModsPage from "./pages/Mods";
import NewsPage from "./pages/News";
import ServersPage from "./pages/Servers";
import SettingsPage from "./pages/Settings";

const STATUS_CLEAR_DELAY_MS = 3000;
const PATCH_NOTE_COUNT = 6;
const SUCCESS_EXIT_CODE = 0;
const GENERIC_ERROR_EXIT_CODE = 1;
const DEFAULT_EXIT_MESSAGE = "The game exited unexpectedly.";
const SIGNAL_NAMES: Record<number, string> = {
  4: "SIGILL",
  6: "SIGABRT",
  7: "SIGBUS",
  8: "SIGFPE",
  11: "SIGSEGV",
  16: "SIGSTKFLT",
};

function getExitReason(code: number | null, signal: number | null): string {
  const hasSignal = signal !== null;
  if (hasSignal) {
    const signalName = SIGNAL_NAMES[signal] ?? signal;
    const reason = `signal ${signalName}`;
    return reason;
  }
  const reason = `code ${code ?? "unknown"}`;
  return reason;
}

function getExitMessage(code: number | null, lastLines: string[] | null): string {
  const lines = lastLines ?? [];
  const hasLines = lines.length > 0;
  const isGenericError = code === GENERIC_ERROR_EXIT_CODE;
  if (!isGenericError) {
    return DEFAULT_EXIT_MESSAGE;
  }
  if (!hasLines) {
    return DEFAULT_EXIT_MESSAGE;
  }
  const message = lines.map((line, index) => `${index + 1}: ${line}`).join("\n");
  return message;
}

function getLaunchInstall(
  selected: Installation | null,
  installations: Installation[],
  serverIp: string | undefined,
  serverVersion: string | undefined,
): Installation | null {
  const hasServerIp = serverIp !== undefined;
  const hasServerVersion = serverVersion !== undefined;
  if (!hasServerIp) {
    return selected;
  }
  if (!hasServerVersion) {
    return selected;
  }
  const builtIn = installations.find(isBuiltInInstallation) ?? null;
  const hasBuiltIn = builtIn !== null;
  if (!hasBuiltIn) {
    return selected;
  }
  const serverInstall: Installation = { ...builtIn, version: serverVersion };
  return serverInstall;
}

function getLaunchActivity(serverIp: string | undefined): Activity {
  const joinsServer = serverIp !== undefined;
  if (!joinsServer) {
    const singleplayer: Activity = { status: "PLAYING_OFFLINE", joinInfo: null };
    return singleplayer;
  }
  const multiplayer: Activity = {
    status: "PLAYING_SERVER",
    joinInfo: { value: serverIp, invited: false },
  };
  return multiplayer;
}

function App() {
  const page = useAppStore((state) => state.page);
  const openedDialog = useAppStore((state) => state.openedDialog);
  const account = useAppStore(getAccount);
  const accounts = useAppStore((state) => state.accounts);
  const installations = useAppStore((state) => state.installations);
  const activeInstall = useAppStore((state) => state.activeInstall);
  const downloadedVersions = useAppStore((state) => state.downloadedVersions);
  const launchWithConsole = useAppStore((state) => state.launcherSettings.launchWithConsole);

  const setPage = useAppStore((state) => state.setPage);
  const setOpenedDialog = useAppStore((state) => state.setOpenedDialog);
  const setAccountMenuOpen = useAppStore((state) => state.setAccountMenuOpen);
  const setAccounts = useAppStore((state) => state.setAccounts);
  const setActiveIndex = useAppStore((state) => state.setActiveIndex);
  const upsertAccount = useAppStore((state) => state.upsertAccount);
  const removeAccountFromList = useAppStore((state) => state.removeAccountFromList);
  const setVersions = useAppStore((state) => state.setVersions);
  const setLaunchingStatus = useAppStore((state) => state.setLaunchingStatus);
  const setAuthLoading = useAppStore((state) => state.setAuthLoading);
  const setAuthUrl = useAppStore((state) => state.setAuthUrl);
  const setStatus = useAppStore((state) => state.setStatus);
  const setNews = useAppStore((state) => state.setNews);
  const setSkinUrl = useAppStore((state) => state.setSkinUrl);
  const setSelectedNote = useAppStore((state) => state.setSelectedNote);
  const setDownloadProgress = useAppStore((state) => state.setDownloadProgress);
  const addDownloadedVersions = useAppStore((state) => state.addDownloadedVersions);
  const loadInstallations = useAppStore((state) => state.loadInstallations);
  const loadLauncherSettings = useAppStore((state) => state.loadLauncherSettings);
  const setCurrentActivity = useAppStore((state) => state.setCurrentActivity);

  useServerPolling();
  useFriendsSync();

  const clearStatusLater = useCallback(() => {
    setTimeout(() => setStatus(""), STATUS_CLEAR_DELAY_MS);
  }, [setStatus]);

  const showStatus = useCallback(
    (text: string) => {
      setStatus(text);
      clearStatusLater();
    },
    [setStatus, clearStatusLater],
  );

  const openPatchNote = useCallback(
    async (note: PatchNote) => {
      const result = await commands.getPatchContent(note.content_path);
      const succeeded = result.ok;
      if (!succeeded) {
        console.error("Failed to fetch content: ", result.error);
        return;
      }
      setSelectedNote({
        title: note.title,
        body: result.value,
        image_url: note.image_url,
        date: note.date,
        entry_type: note.entry_type,
      });
      setPage("news");
    },
    [setPage, setSelectedNote],
  );

  const loadSkin = useCallback(
    (uuid: string) => {
      commands.getSkinUrl(uuid).then((result) => {
        const succeeded = result.ok;
        if (!succeeded) {
          setSkinUrl(null);
          return;
        }
        setSkinUrl(result.value);
      });
    },
    [setSkinUrl],
  );

  useEffect(() => {
    commands.getAllAccounts().then((loadedAccounts) => {
      const hasAccounts = loadedAccounts.length > 0;
      if (!hasAccounts) {
        return;
      }
      setAccounts(loadedAccounts);
      setActiveIndex(0);
      loadSkin(loadedAccounts[0].uuid);
    });
    commands.getPatchNotes(PATCH_NOTE_COUNT).then((result) => {
      const succeeded = result.ok;
      if (!succeeded) {
        console.error("Failed to fetch news:", result.error);
        return;
      }
      setNews(result.value);
    });
    commands.getVersions(false).then((result) => {
      const succeeded = result.ok;
      if (!succeeded) {
        console.error("Failed to fetch versions:", result.error);
        return;
      }
      setVersions(result.value);
    });
  }, [loadSkin, setAccounts, setActiveIndex, setNews, setVersions]);

  useEffect(() => {
    requestAnimationFrame(() => getCurrentWindow().show());
  }, []);

  useEffect(() => {
    loadLauncherSettings();
  }, [loadLauncherSettings]);

  useEffect(() => {
    loadInstallations();
  }, [loadInstallations]);

  useEffect(() => {
    commands.getDownloadedVersions().then(addDownloadedVersions);
  }, [addDownloadedVersions]);

  useEffect(() => {
    const unlisten = events.downloadProgressEvent.listen((event) => {
      setDownloadProgress(event.payload);
    });
    return () => {
      unlisten.then((stop) => stop());
    };
  }, [setDownloadProgress]);

  useEffect(() => {
    const unlisten = events.authUrlEvent.listen((event) => {
      setAuthUrl(event.payload.url);
    });
    return () => {
      unlisten.then((stop) => stop());
    };
  }, [setAuthUrl]);

  const startAddAccount = useCallback(async () => {
    setAccountMenuOpen(false);
    setAuthLoading(true);
    setStatus("Signing in via Microsoft...");
    const result = await commands.addAccount();
    const succeeded = result.ok;
    if (succeeded) {
      const added = result.value;
      upsertAccount(added);
      loadSkin(added.uuid);
      setStatus(`Signed in as ${added.username}`);
    } else {
      setStatus(`Auth failed: ${result.error}`);
    }
    setAuthLoading(false);
    setAuthUrl(null);
  }, [loadSkin, setAccountMenuOpen, setAuthLoading, setAuthUrl, setStatus, upsertAccount]);

  const switchAccount = useCallback(
    (index: number) => {
      setActiveIndex(index);
      setAccountMenuOpen(false);
      const target = accounts[index];
      const hasTarget = target !== undefined;
      if (!hasTarget) {
        return;
      }
      loadSkin(target.uuid);
    },
    [accounts, loadSkin, setAccountMenuOpen, setActiveIndex],
  );

  const removeAccount = useCallback(
    (uuid: string) => {
      commands
        .removeAccount(uuid)
        .catch((error) => console.error("Failed to remove account:", error));
      removeAccountFromList(uuid);
      setAccountMenuOpen(false);
      setSkinUrl(null);
    },
    [removeAccountFromList, setAccountMenuOpen, setSkinUrl],
  );

  const ensureAssets = useCallback(
    async (version: string): Promise<Error | null> => {
      const result = await commands.ensureAssets(version);
      const succeeded = result.ok;
      if (!succeeded) {
        const error = new Error(String(result.error));
        return error;
      }
      addDownloadedVersions([version]);
      return null;
    },
    [addDownloadedVersions],
  );

  const gameRunningRef = useRef(false);

  useEffect(() => {
    const unlisten = events.gameExitedEvent.listen((event) => {
      const wasRunning = gameRunningRef.current;
      if (!wasRunning) {
        return;
      }
      gameRunningRef.current = false;
      setCurrentActivity(ACTIVITY_IDLE);
      const { code, signal, last_lines } = event.payload;
      const exitedCleanly = code === SUCCESS_EXIT_CODE;
      if (exitedCleanly) {
        return;
      }
      const reason = getExitReason(code, signal);
      const message = getExitMessage(code, last_lines);
      setOpenedDialog({
        name: "alert_dialog",
        props: { title: `Game exited (${reason})`, message },
      });
    });
    return () => {
      unlisten.then((stop) => stop());
    };
  }, [setCurrentActivity, setOpenedDialog]);

  const handleLaunch: handleLaunchType = useCallback(
    async ({ serverIp, serverVersion, install } = {}) => {
      const alreadyRunning = gameRunningRef.current;
      if (alreadyRunning) {
        showStatus("Game already running");
        return;
      }

      const selected = install ?? activeInstall;
      const currentInstall = getLaunchInstall(selected, installations, serverIp, serverVersion);
      const hasInstall = currentInstall !== null;
      if (!hasInstall) {
        showStatus("No installation selected");
        return;
      }

      const isDownloaded = downloadedVersions.has(currentInstall.version);
      setLaunchingStatus(isDownloaded ? "checking_assets" : "installing");
      setStatus("Checking assets...");

      const assetsError = await ensureAssets(currentInstall.version);
      const assetsFailed = assetsError !== null;
      if (assetsFailed) {
        setOpenedDialog({
          name: "alert_dialog",
          props: {
            title: "Failed to download assets",
            message: `Failed to download assets for ${currentInstall.version}:\n${assetsError.message}`,
          },
        });
        setDownloadProgress(null);
        setLaunchingStatus(null);
        return;
      }

      setLaunchingStatus("launching");
      setStatus("Launching Pomme...");
      const result = await commands.launchGame(
        currentInstall.id,
        account?.uuid ?? null,
        serverIp ?? null,
        serverVersion ?? null,
        launchWithConsole,
      );
      const launched = result.ok;
      if (launched) {
        gameRunningRef.current = true;
        setCurrentActivity(getLaunchActivity(serverIp));
        setStatus(result.value);
      } else {
        setCurrentActivity(ACTIVITY_IDLE);
        setStatus(result.error);
      }
      setDownloadProgress(null);
      setLaunchingStatus(null);
      clearStatusLater();
    },
    [
      installations,
      ensureAssets,
      activeInstall,
      downloadedVersions,
      setLaunchingStatus,
      setStatus,
      showStatus,
      clearStatusLater,
      setDownloadProgress,
      setOpenedDialog,
      setCurrentActivity,
      account?.uuid,
      launchWithConsole,
    ],
  );

  const handleDialogOpenChange = useCallback(
    (open: boolean) => {
      const closing = !open;
      if (closing) {
        setOpenedDialog(null);
      }
    },
    [setOpenedDialog],
  );

  const hasOpenDialog = openedDialog !== null;

  return (
    <div className="relative isolate flex h-full flex-col overflow-hidden border border-line bg-background">
      <Aurora />
      <Titlebar />

      <div className="flex min-h-0 flex-1 overflow-hidden">
        <Navbar
          startAddAccount={startAddAccount}
          switchAccount={switchAccount}
          removeAccount={removeAccount}
        />

        <main className="flex min-w-0 flex-1 overflow-hidden">
          {page === "home" && (
            <Homepage handleLaunch={handleLaunch} openPatchNote={openPatchNote} />
          )}
          {page === "installations" && <InstallationsPage handleLaunch={handleLaunch} />}
          {page === "news" && <NewsPage openPatchNote={openPatchNote} />}
          {page === "servers" && <ServersPage handleLaunch={handleLaunch} />}
          {page === "friends" && <FriendsPage handleLaunch={handleLaunch} />}
          {page === "mods" && <ModsPage />}
          {page === "settings" && <SettingsPage />}
        </main>
      </div>

      <Dialog.Root open={hasOpenDialog} onOpenChange={handleDialogOpenChange}>
        <Dialog.Portal>
          <Dialog.Overlay className="fixed inset-0 z-40 animate-fade-in bg-black/50 backdrop-blur-md" />
          <Dialog.Content
            aria-describedby={undefined}
            className="fixed top-1/2 left-1/2 z-50 w-[440px] max-w-[calc(100%-40px)] -translate-x-1/2 -translate-y-1/2 animate-dialog-in rounded-2xl border border-line-strong bg-panel/90 p-6 shadow-[0_24px_60px_-20px_rgba(0,0,0,0.7)] backdrop-blur-2xl outline-none"
          >
            {openedDialog?.name === "installation_dialog" && (
              <InstallationDialog {...openedDialog.props} />
            )}
            {openedDialog?.name === "server_dialog" && <ServerDialog {...openedDialog.props} />}
            {openedDialog?.name === "confirm_dialog" && <ConfirmDialog {...openedDialog.props} />}
            {openedDialog?.name === "alert_dialog" && <AlertDialog {...openedDialog.props} />}
            {openedDialog?.name === "add_friend_dialog" && (
              <AddFriendDialog {...openedDialog.props} />
            )}
            {openedDialog?.name === "friend_settings_dialog" && (
              <FriendSettingsDialog {...openedDialog.props} />
            )}
          </Dialog.Content>
        </Dialog.Portal>
      </Dialog.Root>
    </div>
  );
}

export default App;
