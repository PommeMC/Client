import { open as openNativeDialog } from "@tauri-apps/plugin-dialog";
import { useState } from "react";
import { HiChevronDown, HiFolder } from "react-icons/hi2";
import { commands } from "../../bindings";
import { Installation, InstallationError } from "../../bindings/pomme_launcher/installations";
import { isAbsolutePath, normalizeDirectoryName } from "../../lib/helpers";
import { useDropdown } from "../../lib/hooks";
import { useAppStateContext } from "../../lib/state";

export type InstallationDialogProps =
  | { type: "new" }
  | { type: "edit"; installation: Installation }
  | { type: "dupl"; installation: Installation; original_id: string };

function mapInstallationError(error: InstallationError): { name?: string; dir?: string } {
  switch (error.kind) {
    case "InvalidName":
      return { name: "Invalid name" };
    case "NameTooLong":
      return { name: `Name too long (max ${error.detail} characters)` };
    case "InvalidPath":
      return { dir: "Invalid path" };
    case "InvalidCharacter":
      return { dir: `Invalid character: ${error.detail}` };
    case "ReservedName":
      return { dir: `Reserved name: ${error.detail}` };
    case "DirectoryAlreadyExists":
      return { dir: "Directory already exists" };
    case "InstallNotFound":
      return { dir: `Install ${error.detail} not found.` };
    case "Io":
      return { dir: `IO error: ${error.detail}` };
    case "Json":
      return { dir: `JSON error: ${error.detail}` };
    case "Other":
      return { dir: `Unexpected error: ${error.detail}` };
  }
}

export function InstallationDialog({ ...dialogProps }: InstallationDialogProps) {
  const {
    versions,
    setInstallations,
    setActiveInstall,
    setPage,
    setVersions,
    setStatus,
    setDownloadProgress,
    setOpenedDialog,
  } = useAppStateContext();

  function createEmptyInstallation(): Installation {
    return {
      id: "",
      name: "",
      version: versions[0]?.id || "",
      last_played: null,
      directory: "",
      width: 854,
      height: 480,
      is_latest: false,
      created_at: 0,
    };
  }

  const dialogType = dialogProps.type;

  const { ref: versionDropdownRef, ...versionDropdown } = useDropdown();
  const [directoryTouched, setDirectoryTouched] = useState(dialogType === "new" ? false : true);
  const [showSnapshots, setShowSnapshots] = useState(false);

  const [nameError, setNameError] = useState<string | null>(null);
  const [dirError, setDirError] = useState<string | null>(null);
  const [versionError, setVersionError] = useState<string | null>(null);

  const [editingInstall, setEditingInstall] = useState<Installation>(() =>
    dialogType !== "new" ? { ...dialogProps.installation } : createEmptyInstallation(),
  );

  return (
    <div
      className="dialog"
      onClick={(e) => {
        e.stopPropagation();
        if (versionDropdown.isOpen) versionDropdown.close();
      }}
    >
      <h2 className="dialog-title">
        {dialogType === "edit"
          ? "Edit Installation"
          : dialogType === "dupl"
            ? "Duplicate Installation"
            : "New Installation"}
      </h2>

      <div className="dialog-fields">
        <div className="dialog-field">
          <label>NAME</label>
          <input
            disabled={editingInstall.is_latest}
            value={editingInstall.name}
            onChange={(e) => {
              const name = e.target.value;
              setNameError(null);
              if (!directoryTouched) setDirError(null);
              setEditingInstall((prev) => {
                if (!prev) return prev;
                return {
                  ...prev,
                  name,
                  directory: directoryTouched ? prev.directory : normalizeDirectoryName(name),
                };
              });
            }}
            placeholder="My Installation"
            autoFocus
          />
          <span className={`dialog-field-info ${nameError ? "error" : ""}`}>{nameError}</span>
        </div>
        <div className="dialog-field">
          <label>VERSION</label>
          <div className="custom-select-wrapper" ref={versionDropdownRef}>
            <button className="custom-select" onClick={versionDropdown.toggle} type="button">
              <span>{editingInstall.version}</span>
              <HiChevronDown
                className={`custom-select-arrow ${versionDropdown.isOpen ? "open" : ""}`}
              />
            </button>
            {versionDropdown.isOpen && (
              <div className="custom-select-dropdown">
                <label className="custom-select-toggle">
                  <input
                    type="checkbox"
                    checked={showSnapshots}
                    onChange={(e) => {
                      setShowSnapshots(e.target.checked);
                      commands.getVersions(e.target.checked).then((res) => {
                        if (res.ok) {
                          setVersions(res.value);
                        } else {
                          console.error("Failed to fetch versions: ", res.error);
                        }
                      });
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
                        setEditingInstall((prev) => ({ ...prev, version: v.id }));
                        versionDropdown.close();
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
          <span className={`dialog-field-info error`}>{versionError || ""}</span>
        </div>
        <div className="dialog-field">
          <label>GAME DIRECTORY</label>
          <div className="dialog-browse">
            <input
              value={editingInstall.directory}
              onChange={(e) => {
                const dirname = e.target.value;
                setDirError(null);
                setDirectoryTouched(dirname !== "");
                setEditingInstall((prev) => ({ ...prev, directory: dirname }));
              }}
              placeholder="my-installation"
            />
            <button
              className="dialog-browse-btn"
              onClick={async () => {
                const path = await openNativeDialog({ directory: true });
                if (path) {
                  setDirectoryTouched(true);
                  setEditingInstall((prev) => ({ ...prev, directory: path }));
                }
              }}
            >
              <HiFolder />
            </button>
          </div>
          <span className={`dialog-field-info ${dirError ? "error" : ""}`}>
            {dirError ||
              (!isAbsolutePath(editingInstall.directory) &&
                editingInstall.directory !== normalizeDirectoryName(editingInstall.directory) &&
                "Will be created as: " +
                  normalizeDirectoryName(editingInstall.directory || "my-installation"))}
          </span>
        </div>
        <div className="dialog-field">
          <label>RESOLUTION</label>
          <div className="dialog-resolution">
            <input
              type="number"
              value={editingInstall.width}
              onChange={(e) =>
                setEditingInstall((prev) => ({
                  ...prev,
                  width: parseInt(e.target.value) || 854,
                }))
              }
              placeholder="854"
            />
            <span className="dialog-resolution-x">×</span>
            <input
              type="number"
              value={editingInstall.height}
              onChange={(e) =>
                setEditingInstall((prev) => ({
                  ...prev,
                  height: parseInt(e.target.value) || 480,
                }))
              }
              placeholder="480"
            />
          </div>
        </div>
      </div>

      <div className="dialog-actions">
        <button className="dialog-cancel" onClick={() => setOpenedDialog(null)}>
          Cancel
        </button>
        <button
          className="dialog-save"
          onClick={async () => {
            const editedInstall: Installation = {
              ...editingInstall,
              name: editingInstall.name || "My Installation",
              version: editingInstall.version || versions[0]?.id || "",
              width: editingInstall.width || 854,
              height: editingInstall.height || 480,
            };
            editedInstall.directory = isAbsolutePath(editingInstall.directory)
              ? editingInstall.directory
              : normalizeDirectoryName(editingInstall.directory || editedInstall.name);

            if (editingInstall.version === "") {
              setVersionError("Invalid version");
              return;
            }

            if (dialogType !== "edit") {
              const installResult = await (dialogType === "new"
                ? commands.createInstallation(editedInstall)
                : commands.duplicateInstallation(dialogProps.original_id, editedInstall));

              if (!installResult.ok) {
                const mapped = mapInstallationError(installResult.error);
                if (mapped.name) setNameError(mapped.name);
                if (mapped.dir) setDirError(mapped.dir);
                return;
              }
              const install = installResult.value;
              setInstallations((prev) => [...prev, install]);
              setActiveInstall(install);

              setOpenedDialog(null);
              setPage("home");
              setDownloadProgress({ downloaded: 0, total: 1, status: "Starting install..." });

              const ensureAssetsResult = await commands.ensureAssets(install.version);
              if (ensureAssetsResult.ok) {
                setStatus(`${install.name} ready`);
              } else {
                setStatus(`Install failed: ${ensureAssetsResult.error}`);
              }

              setDownloadProgress(null);
              setTimeout(() => setStatus(""), 3000);
            } else {
              const editInstallResult = await commands.editInstallation(
                editingInstall.id,
                editedInstall,
              );
              if (!editInstallResult.ok) {
                const mapped = mapInstallationError(editInstallResult.error);
                if (mapped.name) setNameError(mapped.name);
                if (mapped.dir) setDirError(mapped.dir);
                return;
              }
              setInstallations((prev) =>
                prev.map((i) => (i.id === editingInstall.id ? editedInstall : i)),
              );
              setActiveInstall(editedInstall);
              setOpenedDialog(null);
            }
          }}
        >
          {dialogType === "new" ? "Install" : dialogType === "edit" ? "Save" : "Duplicate"}
        </button>
      </div>
    </div>
  );
}
