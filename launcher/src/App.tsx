import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useCallback, useEffect, useRef } from "react";
import Navbar from "./components/Navbar";
import Titlebar from "./components/Titlebar";
import { ConfirmDialog } from "./components/dialogs/ConfirmDialog.tsx";
import { InstallationDialog } from "./components/dialogs/InstallationDialog.tsx";
import { useAppStateContext } from "./lib/state";
import {
  AuthAccount,
  DownloadProgress,
  GameVersion,
  Installation,
  InstallationError,
  PatchNote,
} from "./lib/types";
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
    server,
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
    invokeCreateInstallation,
    invokeDeleteInstallation,
    invokeDuplicateInstallation,
    invokeEditInstallation,
    activeInstall,
    setActiveInstall,
    setInstallations,
    setDownloadedVersions,
  } = useAppStateContext();

  const { setIsOpen: setAccountDropdownOpen } = accountDropdown;

  const openPatchNote = useCallback(
    async (note: PatchNote) => {
      try {
        const body = await invoke<string>("get_patch_content", {
          contentPath: note.content_path,
        });
        setSelectedNote({
          title: note.title,
          body,
          image_url: note.image_url,
        });
        setPage("news");
      } catch (e) {
        console.error("Failed to fetch content:", e);
      }
    },
    [setPage, setSelectedNote],
  );

  const loadSkin = useCallback(
    (uuid: string) => {
      invoke<string>("get_skin_url", { uuid })
        .then(setSkinUrl)
        .catch(() => setSkinUrl(null));
    },
    [setSkinUrl],
  );

  useEffect(() => {
    invoke<AuthAccount[]>("get_all_accounts").then((accs) => {
      if (accs.length > 0) {
        setAccounts(accs);
        setActiveIndex(0);
        loadSkin(accs[0].uuid);
      }
    });
    invoke<PatchNote[]>("get_patch_notes", { count: 6 })
      .then(setNews)
      .catch((e) => console.error("Failed to fetch news:", e));
    invoke<GameVersion[]>("get_versions", { showSnapshots: false })
      .then(setVersions)
      .catch((e) => console.error("Failed to fetch versions:", e));
  }, [loadSkin, setAccounts, setActiveIndex, setNews, setVersions]);

  useEffect(() => {
    requestAnimationFrame(() => getCurrentWindow().show());
  }, []);

  useEffect(() => {
    const unlisten = listen<DownloadProgress>("download-progress", (event) => {
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
    try {
      const acc = await invoke<AuthAccount>("add_account");
      setAccounts((prev) => {
        const filtered = prev.filter((a) => a.uuid !== acc.uuid);
        return [...filtered, acc];
      });
      setActiveIndex(accounts.filter((a) => a.uuid !== acc.uuid).length);
      loadSkin(acc.uuid);
      setStatus(`Signed in as ${acc.username}`);
    } catch (e) {
      setStatus(`Auth failed: ${e}`);
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
      invoke("remove_account", { uuid }).catch((e) =>
        console.error("Failed to remove account:", e),
      );
      setAccounts((prev) => prev.filter((a) => a.uuid !== uuid));
      setActiveIndex(0);
      setAccountDropdownOpen(false);
      setSkinUrl(null);
    },
    [setAccountDropdownOpen, setAccounts, setActiveIndex, setSkinUrl],
  );

  const handleLaunch = useCallback(async () => {
    if (!activeInstall) {
      setStatus("No installation selected");
      setTimeout(() => setStatus(""), 3000);
      return;
    }

    setStatus("Checking assets...");
    try {
      if (downloadedVersions.has(activeInstall.version)) {
        setLaunchingStatus("checking_assets");
      } else {
        setLaunchingStatus("installing");
      }
      await invoke("ensure_assets", { version: activeInstall.version });
      setDownloadProgress(null);
      setLaunchingStatus("launching");
      setStatus("Launching Pomme...");
      const result = await invoke<string>("launch_game", {
        uuid: account?.uuid || null,
        server: server || null,
        debugEnabled: launcherSettings.launchWithConsole || null,
        version: activeInstall.version,
      });
      setStatus(result);
      setLaunchingStatus(null);
      setDownloadedVersions((prev) => prev.add(activeInstall.version));
    } catch (e) {
      setDownloadProgress(null);
      setStatus(`${e}`);
      setLaunchingStatus(null);
    }
    setTimeout(() => {
      setStatus("");
    }, 3000);
  }, [
    activeInstall,
    setLaunchingStatus,
    setStatus,
    setDownloadProgress,
    downloadedVersions,
    account?.uuid,
    server,
    launcherSettings.launchWithConsole,
  ]);

  const dialogDragStartedInside = useRef(false);

  const createInstallation = async (
    payload: Installation,
  ): Promise<[Installation, null] | [null, InstallationError]> => {
    try {
      const inst = await invokeCreateInstallation(payload);
      setInstallations((prev) => [...prev, inst]);
      return [inst, null];
    } catch (e) {
      console.error("Failed to create installation", e);
      return [null, e as InstallationError];
    }
  };

  const deleteInstallation = async (install_id: string): Promise<null | InstallationError> => {
    try {
      await invokeDeleteInstallation(install_id);
      setInstallations((prev) => prev.filter((inst) => inst.id !== install_id));
      return null;
    } catch (e) {
      console.error("Failed to delete installation", e);
      return e as InstallationError;
    }
  };

  const duplicateInstallation = async (
    install_id: string,
    new_payload: Installation,
  ): Promise<[Installation, null] | [null, InstallationError]> => {
    try {
      const inst = await invokeDuplicateInstallation(install_id, new_payload);
      setInstallations((prev) => [...prev, inst]);
      return [inst, null];
    } catch (e) {
      console.error("Failed to duplicate installation", e);
      return [null, e as InstallationError];
    }
  };

  const editInstallation = async (
    install_id: string,
    new_payload: Installation,
  ): Promise<null | InstallationError> => {
    try {
      const inst = await invokeEditInstallation(install_id, new_payload);
      setInstallations((prev) => prev.map((i) => (i.id === install_id ? inst : i)));
      return null;
    } catch (e) {
      console.error("Failed to edit installation", e);
      return e as InstallationError;
    }
  };

  useEffect(() => {
    invoke<Installation[]>("load_installations")
      .then((installs) => {
        setInstallations(installs);
        if (!activeInstall) setActiveInstall(installs[0]);
      })
      .catch((e) => setStatus("Failed to load installations: " + e));
  }, [setInstallations, setActiveInstall, activeInstall, setStatus]);

  useEffect(() => {
    invoke<string[]>("get_downloaded_versions")
      .then((versions) => {
        setDownloadedVersions((prev) => new Set([...prev, ...versions]));
      })
      .catch((e) => console.error("Failed to load downloaded versions: " + e));
  }, [setDownloadedVersions, setStatus]);

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

          {page === "installations" && (
            <InstallationsPage deleteInstallation={deleteInstallation} />
          )}

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
          {openedDialog.name === "installation" && (
            <InstallationDialog
              {...openedDialog.props}
              createInstallation={createInstallation}
              duplicateInstallation={duplicateInstallation}
              editInstallation={editInstallation}
            />
          )}
          {openedDialog.name === "confirm_dialog" && <ConfirmDialog {...openedDialog.props} />}
        </div>
      )}
    </div>
  );
}

export default App;
