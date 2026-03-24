import { useCallback, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { HiChevronDown, HiFolder } from "react-icons/hi2";
import { AuthAccount, GameVersion, PatchNote } from "./lib/types";
import Homepage from "./pages/Home";
import InstallationsPage from "./pages/Installations";
import { useAppStateContext } from "./lib/state";
import Navbar from "./components/Navbar";
import ModsPage from "./pages/Mods";
import ServersPage from "./pages/Servers";
import FriendsPage from "./pages/Friends";
import NewsPage from "./pages/News";
import SettingsPage from "./pages/Settings";
import Titlebar from "./components/Titlebar";

function App() {
  const {
    account,
    page,
    setPage,
    accounts,
    setAccounts,
    setActiveIndex,
    setAccountDropdownOpen,
    server,

    setInstallations,
    editingInstall,
    setEditingInstall,
    dialogVersionOpen,
    setDialogVersionOpen,
    selectedVersion,
    versions,
    setVersions,
    showSnapshots,
    setShowSnapshots,
    setLaunching,
    setAuthLoading,
    setStatus,
    setNews,
    setSkinUrl,
    setSelectedNote,
    useConsole,
  } = useAppStateContext();

  const openPatchNote = useCallback(
    async (note: PatchNote) => {
      try {
        const body = await invoke<string>("get_patch_content", {
          contentPath: note.content_path,
        });
        setSelectedNote({ title: note.title, body });
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
      invoke("remove_account", { uuid });
      setAccounts((prev) => prev.filter((a) => a.uuid !== uuid));
      setActiveIndex(0);
      setAccountDropdownOpen(false);
      setSkinUrl(null);
    },
    [setAccountDropdownOpen, setAccounts, setActiveIndex, setSkinUrl],
  );

  const handleLaunch = useCallback(async () => {
    setLaunching(true);
    setStatus("Checking assets...");
    try {
      await invoke("ensure_assets", { version: selectedVersion });
      setStatus("Launching POMC...");
      const result = await invoke<string>("launch_game", {
        uuid: account?.uuid || null,
        server: server || null,
        debugEnabled: useConsole || null,
      });
      setStatus(result);
    } catch (e) {
      setStatus(`${e}`);
    }
    setTimeout(() => {
      setLaunching(false);
      setStatus("");
    }, 3000);
  }, [setLaunching, setStatus, selectedVersion, account?.uuid, server, useConsole]);

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

          {page === "installations" && <InstallationsPage />}

          {page === "news" && <NewsPage openPatchNote={openPatchNote} />}

          {page === "servers" && <ServersPage />}

          {page === "friends" && <FriendsPage />}

          {page === "mods" && <ModsPage />}

          {page === "settings" && <SettingsPage />}
        </main>
      </div>

      {editingInstall && (
        <div
          className="dialog-overlay"
          onClick={() => {
            setEditingInstall(null);
            setDialogVersionOpen(false);
          }}
        >
          <div
            className="dialog"
            onClick={(e) => {
              e.stopPropagation();
              if (dialogVersionOpen) setDialogVersionOpen(false);
            }}
          >
            <h2 className="dialog-title">
              {editingInstall.id ? "Edit Installation" : "New Installation"}
            </h2>

            <div className="dialog-fields">
              <div className="dialog-field">
                <label>NAME</label>
                <input
                  value={editingInstall.name}
                  onChange={(e) => setEditingInstall({ ...editingInstall, name: e.target.value })}
                  placeholder="My Installation"
                  autoFocus
                />
              </div>
              <div className="dialog-field">
                <label>VERSION</label>
                <div className="custom-select-wrapper">
                  <button
                    className="custom-select"
                    onClick={() => setDialogVersionOpen(!dialogVersionOpen)}
                    type="button"
                  >
                    <span>{editingInstall.version}</span>
                    <HiChevronDown
                      className={`custom-select-arrow ${dialogVersionOpen ? "open" : ""}`}
                    />
                  </button>
                  {dialogVersionOpen && (
                    <div className="custom-select-dropdown" onClick={(e) => e.stopPropagation()}>
                      <label className="custom-select-toggle">
                        <input
                          type="checkbox"
                          checked={showSnapshots}
                          onChange={(e) => {
                            setShowSnapshots(e.target.checked);
                            invoke<GameVersion[]>("get_versions", {
                              showSnapshots: e.target.checked,
                            }).then(setVersions);
                          }}
                        />
                        <span>Show snapshots</span>
                      </label>
                      <div className="custom-select-list">
                        {versions.map((v) => (
                          <button
                            key={v.id}
                            className={`custom-select-item ${v.id === editingInstall.version ? "active" : ""}`}
                            onClick={() => {
                              setEditingInstall({
                                ...editingInstall,
                                version: v.id,
                              });
                              setDialogVersionOpen(false);
                            }}
                          >
                            <span>{v.id}</span>
                            {v.version_type !== "release" && (
                              <span className="custom-select-tag">{v.version_type}</span>
                            )}
                          </button>
                        ))}
                      </div>
                    </div>
                  )}
                </div>
              </div>
              <div className="dialog-field">
                <label>GAME DIRECTORY</label>
                <div className="dialog-browse">
                  <input
                    value={editingInstall.directory}
                    onChange={(e) =>
                      setEditingInstall({ ...editingInstall, directory: e.target.value })
                    }
                    placeholder="default"
                  />
                  <button
                    className="dialog-browse-btn"
                    onClick={async () => {
                      const path = await openDialog({ directory: true });
                      if (path) {
                        setEditingInstall({
                          ...editingInstall,
                          directory: path as string,
                        });
                      }
                    }}
                  >
                    <HiFolder />
                  </button>
                </div>
              </div>
              <div className="dialog-field">
                <label>RESOLUTION</label>
                <div className="dialog-resolution">
                  <input
                    type="number"
                    value={editingInstall.width}
                    onChange={(e) =>
                      setEditingInstall({
                        ...editingInstall,
                        width: parseInt(e.target.value) || 854,
                      })
                    }
                    placeholder="854"
                  />
                  <span className="dialog-resolution-x">×</span>
                  <input
                    type="number"
                    value={editingInstall.height}
                    onChange={(e) =>
                      setEditingInstall({
                        ...editingInstall,
                        height: parseInt(e.target.value) || 480,
                      })
                    }
                    placeholder="480"
                  />
                </div>
              </div>
            </div>

            <div className="dialog-actions">
              <button className="dialog-cancel" onClick={() => setEditingInstall(null)}>
                Cancel
              </button>
              <button
                className="dialog-save"
                onClick={async () => {
                  const isNew = !editingInstall.id;
                  const install = {
                    ...editingInstall,
                    id: editingInstall.id || Date.now().toString(36),
                    name: editingInstall.name || "Installation",
                    directory: editingInstall.directory || "default",
                  };
                  setInstallations((prev) => {
                    const filtered = prev.filter((i) => i.id !== install.id);
                    return [...filtered, install];
                  });
                  setEditingInstall(null);
                  if (isNew) {
                    setStatus(`Installing ${install.version}...`);
                    try {
                      await invoke("ensure_assets", { version: install.version });
                      setStatus(`${install.name} ready`);
                    } catch (e) {
                      setStatus(`Install failed: ${e}`);
                    }
                    setTimeout(() => setStatus(""), 3000);
                  }
                }}
              >
                {editingInstall.id ? "Save" : "Install"}
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

export default App;
