import { getCurrentWindow } from "@tauri-apps/api/window";
import { useCallback, useEffect, useRef } from "react";

import { commands, events } from "./bindings";
import { PatchNote } from "./bindings/pomme_launcher/commands";
import { useAppStateContext } from "./lib/state";
import { handleLaunchType } from "./lib/types";

import Navbar from "./components/Navbar";
import Titlebar from "./components/Titlebar";
import AlertDialog from "./components/dialogs/AlertDialog";
import { ConfirmDialog } from "./components/dialogs/ConfirmDialog";
import { InstallationDialog } from "./components/dialogs/InstallationDialog";
import { ServerDialog } from "./components/dialogs/ServerDialog";

import FriendsPage from "./pages/Friends";
import Homepage from "./pages/Home";
import InstallationsPage from "./pages/Installations";
import ModsPage from "./pages/Mods";
import NewsPage from "./pages/News";
import ServersPage from "./pages/Servers";
import SettingsPage from "./pages/Settings";

function App() {
  const {
    account,
    accountDropdown,
    page,
    setPage,
    accounts,
    setAccounts,
    setActiveIndex,
    setVersions,
    downloadedVersions,
    setLaunchingStatus,
    setAuthLoading,
    setStatus,
    setNews,
    setSkinUrl,
    setSelectedNote,
    setDownloadProgress,
    openedDialog,
    setOpenedDialog,
    launcherSettings,
    activeInstall,
    setActiveInstall,
    installations,
    setInstallations,
    setDownloadedVersions,
  } = useAppStateContext();

  const { setIsOpen: setAccountDropdownOpen } = accountDropdown;

  const openPatchNote = useCallback(
    async (note: PatchNote) => {
      const res = await commands.getPatchContent(note.content_path);
      if (res.ok) {
        setSelectedNote({
          title: note.title,
          body: res.value,
          image_url: note.image_url,
          date: note.date,
          entry_type: note.entry_type,
        });
        setPage("news");
      } else {
        console.error("Failed to fetch content: ", res.error);
      }
    },
    [setPage, setSelectedNote],
  );

  const loadSkin = useCallback(
    (uuid: string) => {
      commands.getSkinUrl(uuid).then((res) => {
        if (res.ok) setSkinUrl(res.value);
        else setSkinUrl(null);
      });
    },
    [setSkinUrl],
  );

  useEffect(() => {
    commands.getAllAccounts().then((accs) => {
      if (accs.length > 0) {
        setAccounts(accs);
        setActiveIndex(0);
        loadSkin(accs[0].uuid);
      }
    });
    commands.getPatchNotes(6).then((res) => {
      if (res.ok) setNews(res.value);
      else console.error("Failed to fetch news:", res.error);
    });
    commands.getVersions(false).then((res) => {
      if (res.ok) setVersions(res.value);
      else console.error("Failed to fetch versions:", res.error);
    });
  }, [loadSkin, setAccounts, setActiveIndex, setNews, setVersions]);

  useEffect(() => {
    requestAnimationFrame(() => getCurrentWindow().show());
  }, []);

  useEffect(() => {
    const unlisten = events.downloadProgressEvent.listen((event) => {
      setDownloadProgress(event.payload);
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [setDownloadProgress]);

  const startAddAccount = useCallback(async () => {
    setAccountDropdownOpen(false);
    setAuthLoading(true);
    setStatus("Signing in via Microsoft...");
    const res = await commands.addAccount();
    if (res.ok) {
      const acc = res.value;
      setAccounts((prev) => {
        const filtered = prev.filter((a) => a.uuid !== acc.uuid);
        return [...filtered, acc];
      });
      setActiveIndex(accounts.filter((a) => a.uuid !== acc.uuid).length);
      loadSkin(acc.uuid);
      setStatus(`Signed in as ${acc.username}`);
    } else {
      setStatus(`Auth failed: ${res.error}`);
    }
    setAuthLoading(false);
  }, [
    accounts,
    loadSkin,
    setAccountDropdownOpen,
    setAccounts,
    setActiveIndex,
    setAuthLoading,
    setStatus,
  ]);

  const switchAccount = useCallback(
    (index: number) => {
      setActiveIndex(index);
      setAccountDropdownOpen(false);
      if (accounts[index]) {
        loadSkin(accounts[index].uuid);
      }
    },
    [accounts, loadSkin, setAccountDropdownOpen, setActiveIndex],
  );

  const removeAccount = useCallback(
    (uuid: string) => {
      commands.removeAccount(uuid).catch((e) => console.error("Failed to remove account:", e));
      setAccounts((prev) => prev.filter((a) => a.uuid !== uuid));
      setActiveIndex(0);
      setAccountDropdownOpen(false);
      setSkinUrl(null);
    },
    [setAccountDropdownOpen, setAccounts, setActiveIndex, setSkinUrl],
  );

  const ensureAssets = useCallback(
    async (version: string): Promise<Error | null> => {
      const res = await commands.ensureAssets(version);
      if (!res.ok) {
        return new Error(String(res.error));
      }
      setDownloadedVersions((prev) => new Set([...prev, version]));
      return null;
    },
    [setDownloadedVersions],
  );

  const handleLaunch: handleLaunchType = useCallback(
    async ({ serverIp, serverVersion, install } = {}) => {
      let currentInstall = install ?? activeInstall;
      if (serverVersion && serverIp) {
        const candidate =
          installations.find((i) => i.id === "latest-release" || i.id === "latest-snapshot") ??
          null;
        if (candidate) {
          currentInstall = { ...candidate, version: serverVersion };
        }
      }

      if (!currentInstall) {
        setStatus("No installation selected");
        setTimeout(() => setStatus(""), 3000);
        return;
      }

      if (downloadedVersions.has(currentInstall.version)) {
        setLaunchingStatus("checking_assets");
      } else {
        setLaunchingStatus("installing");
      }
      setStatus("Checking assets...");

      const err = await ensureAssets(currentInstall.version);
      if (err instanceof Error) {
        setOpenedDialog({
          name: "alert_dialog",
          props: {
            title: "Failed to download assets",
            message: `Failed to download assets for ${currentInstall.version}:\n${err.message}`,
          },
        });
        setDownloadProgress(null);
        setLaunchingStatus(null);
        return;
      }

      await events.gameExitedEvent.once((event) => {
        const { code, signal, last_lines } = event.payload;
        if (code === 0) return;
        const SIGNAL_NAMES: Record<number, string> = {
          4: "SIGILL",
          6: "SIGABRT",
          7: "SIGBUS",
          8: "SIGFPE",
          11: "SIGSEGV",
          16: "SIGSTKFLT",
        };
        const reason =
          signal !== null
            ? `signal ${SIGNAL_NAMES[signal] ?? signal}`
            : `code ${code ?? "unknown"}`;
        const message =
          code === 1 && last_lines && last_lines.length > 0
            ? last_lines.map((line, i) => `${i + 1}: ${line}`).join("\n")
            : "The game exited unexpectedly.";
        setOpenedDialog({
          name: "alert_dialog",
          props: {
            title: `Game exited (${reason})`,
            message,
          },
        });
      });

      setLaunchingStatus("launching");
      setStatus("Launching Pomme...");
      const res = await commands.launchGame(
        currentInstall.id,
        account?.uuid ?? null,
        serverIp ?? null,
        serverVersion ?? null,
        launcherSettings.launchWithConsole ?? null,
      );
      if (res.ok) {
        setStatus(res.value);
      } else {
        setStatus(res.error);
      }
      setDownloadProgress(null);
      setLaunchingStatus(null);
      setTimeout(() => setStatus(""), 3000);
    },
    [
      installations,
      ensureAssets,
      activeInstall,
      downloadedVersions,
      setLaunchingStatus,
      setStatus,
      setDownloadProgress,
      setOpenedDialog,
      account?.uuid,
      launcherSettings.launchWithConsole,
    ],
  );

  const dialogDragStartedInside = useRef(false);

  useEffect(() => {
    commands.loadInstallations().then((res) => {
      if (res.ok) {
        setInstallations(res.value);
        setActiveInstall((prev) => prev ?? res.value[0]);
      } else {
        setStatus("Failed to load installations: " + res.error);
      }
    });
  }, [setInstallations, setActiveInstall, setStatus]);

  useEffect(() => {
    commands.getDownloadedVersions().then((versions) => {
      setDownloadedVersions((prev) => new Set([...prev, ...versions]));
    });
  }, [setDownloadedVersions]);

  return (
    <div className="app">
      <Titlebar />

      <div className="layout">
        <Navbar
          startAddAccount={startAddAccount}
          switchAccount={switchAccount}
          removeAccount={removeAccount}
        />

        <main className="content">
          {page === "home" && (
            <Homepage handleLaunch={handleLaunch} openPatchNote={openPatchNote} />
          )}

          {page === "installations" && <InstallationsPage handleLaunch={handleLaunch} />}

          {page === "news" && <NewsPage openPatchNote={openPatchNote} />}

          {page === "servers" && <ServersPage handleLaunch={handleLaunch} />}

          {page === "friends" && <FriendsPage />}

          {page === "mods" && <ModsPage />}

          {page === "settings" && <SettingsPage />}
        </main>
      </div>

      {openedDialog !== null && (
        <div
          className="dialog-overlay"
          onMouseDown={(e) => {
            dialogDragStartedInside.current = e.target !== e.currentTarget;
          }}
          onClick={(e) => {
            if (e.target === e.currentTarget && !dialogDragStartedInside.current) {
              setOpenedDialog(null);
            }
          }}
        >
          {openedDialog.name === "installation_dialog" && (
            <InstallationDialog {...openedDialog.props} />
          )}
          {openedDialog.name === "server_dialog" && <ServerDialog {...openedDialog.props} />}
          {openedDialog.name === "confirm_dialog" && <ConfirmDialog {...openedDialog.props} />}
          {openedDialog.name === "alert_dialog" && <AlertDialog {...openedDialog.props} />}
        </div>
      )}
    </div>
  );
}

export default App;
