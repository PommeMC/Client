import { open as openNativeDialog } from "@tauri-apps/plugin-dialog";
import { Check, ChevronDown, Folder } from "lucide-react";
import { Dialog, DropdownMenu } from "radix-ui";
import { useState } from "react";
import { commands } from "../../bindings";
import { Installation, InstallationError } from "../../bindings/pomme_launcher/installations";
import { isAbsolutePath, normalizeDirectoryName } from "../../lib/helpers";
import { useAppStore } from "../../lib/store";

const DEFAULT_WIDTH = 854;
const DEFAULT_HEIGHT = 480;
const DEFAULT_NAME = "My Installation";
const DEFAULT_DIRECTORY = "my-installation";
const RELEASE_VERSION_TYPE = "release";
const ICON_SIZE = 14;
const CHECK_ICON_SIZE = 10;
const resolutionInputClass =
  "field w-[100px] text-center tabular-nums [appearance:textfield] [&::-webkit-inner-spin-button]:appearance-none [&::-webkit-outer-spin-button]:appearance-none";

export type InstallationDialogProps =
  | { type: "new" }
  | { type: "edit"; installation: Installation }
  | { type: "dupl"; installation: Installation; original_id: string };

const DIALOG_TITLES: Record<InstallationDialogProps["type"], string> = {
  new: "New Installation",
  edit: "Edit Installation",
  dupl: "Duplicate Installation",
};

const SAVE_LABELS: Record<InstallationDialogProps["type"], string> = {
  new: "Install",
  edit: "Save",
  dupl: "Duplicate",
};

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

function getDirectoryHint(directory: string): string {
  const isAbsolute = isAbsolutePath(directory);
  if (isAbsolute) {
    return "";
  }
  const normalized = normalizeDirectoryName(directory);
  const alreadyNormalized = directory === normalized;
  if (alreadyNormalized) {
    return "";
  }
  const hint = `Will be created as: ${normalizeDirectoryName(directory || DEFAULT_DIRECTORY)}`;
  return hint;
}

export function InstallationDialog({ ...dialogProps }: InstallationDialogProps) {
  const versions = useAppStore((state) => state.versions);
  const setVersions = useAppStore((state) => state.setVersions);
  const addInstallation = useAppStore((state) => state.addInstallation);
  const replaceInstallation = useAppStore((state) => state.replaceInstallation);
  const setPage = useAppStore((state) => state.setPage);
  const setStatus = useAppStore((state) => state.setStatus);
  const setDownloadProgress = useAppStore((state) => state.setDownloadProgress);
  const setOpenedDialog = useAppStore((state) => state.setOpenedDialog);

  function createEmptyInstallation(): Installation {
    const empty: Installation = {
      id: "",
      name: "",
      version: versions[0]?.id || "",
      last_played: null,
      directory: "",
      width: DEFAULT_WIDTH,
      height: DEFAULT_HEIGHT,
      is_latest: false,
      created_at: 0,
    };
    return empty;
  }

  const dialogType = dialogProps.type;
  const isNew = dialogType === "new";
  const isEdit = dialogType === "edit";

  const [directoryTouched, setDirectoryTouched] = useState(!isNew);
  const [showSnapshots, setShowSnapshots] = useState(false);

  const [nameError, setNameError] = useState<string | null>(null);
  const [dirError, setDirError] = useState<string | null>(null);
  const [versionError, setVersionError] = useState<string | null>(null);

  const [editingInstall, setEditingInstall] = useState<Installation>(() => {
    const initial = isNew ? createEmptyInstallation() : { ...dialogProps.installation };
    return initial;
  });

  const changeName = (name: string) => {
    setNameError(null);
    if (!directoryTouched) {
      setDirError(null);
    }
    setEditingInstall((previous) => {
      const directory = directoryTouched ? previous.directory : normalizeDirectoryName(name);
      const next: Installation = { ...previous, name, directory };
      return next;
    });
  };

  const changeDirectory = (directory: string) => {
    setDirError(null);
    setDirectoryTouched(directory !== "");
    setEditingInstall((previous) => ({ ...previous, directory }));
  };

  const browseDirectory = async () => {
    const path = await openNativeDialog({ directory: true });
    const picked = typeof path === "string";
    if (!picked) {
      return;
    }
    setDirectoryTouched(true);
    setEditingInstall((previous) => ({ ...previous, directory: path }));
  };

  const toggleSnapshots = (checked: boolean) => {
    setShowSnapshots(checked);
    commands.getVersions(checked).then((result) => {
      const succeeded = result.ok;
      if (!succeeded) {
        console.error("Failed to fetch versions: ", result.error);
        return;
      }
      setVersions(result.value);
    });
  };

  const selectVersion = (version: string) => {
    setEditingInstall((previous) => ({ ...previous, version }));
  };

  const applyErrors = (error: InstallationError) => {
    const { name, dir } = mapInstallationError(error);
    const hasNameError = name !== undefined;
    const hasDirError = dir !== undefined;
    if (hasNameError) {
      setNameError(name);
    }
    if (hasDirError) {
      setDirError(dir);
    }
  };

  const createAndInstall = async (editedInstall: Installation) => {
    const installResult = isNew
      ? await commands.createInstallation(editedInstall)
      : await commands.duplicateInstallation(
          (dialogProps as { original_id: string }).original_id,
          editedInstall,
        );
    const succeeded = installResult.ok;
    if (!succeeded) {
      applyErrors(installResult.error);
      return;
    }
    const install = installResult.value;
    addInstallation(install);

    setOpenedDialog(null);
    setPage("home");
    setDownloadProgress({ downloaded: 0, total: 1, status: "Starting install..." });

    const ensureAssetsResult = await commands.ensureAssets(install.version);
    const assetsReady = ensureAssetsResult.ok;
    if (assetsReady) {
      setStatus(`${install.name} ready`);
    } else {
      setStatus(`Install failed: ${ensureAssetsResult.error}`);
    }

    setDownloadProgress(null);
    setTimeout(() => setStatus(""), STATUS_CLEAR_DELAY_MS);
  };

  const saveEdit = async (editedInstall: Installation) => {
    const editResult = await commands.editInstallation(editingInstall.id, editedInstall);
    const succeeded = editResult.ok;
    if (!succeeded) {
      applyErrors(editResult.error);
      return;
    }
    replaceInstallation(editedInstall);
    setOpenedDialog(null);
  };

  const save = async () => {
    const editedInstall: Installation = {
      ...editingInstall,
      name: editingInstall.name || DEFAULT_NAME,
      version: editingInstall.version || versions[0]?.id || "",
      width: editingInstall.width || DEFAULT_WIDTH,
      height: editingInstall.height || DEFAULT_HEIGHT,
    };
    const isAbsolute = isAbsolutePath(editingInstall.directory);
    editedInstall.directory = isAbsolute
      ? editingInstall.directory
      : normalizeDirectoryName(editingInstall.directory || editedInstall.name);

    const hasVersion = editingInstall.version !== "";
    if (!hasVersion) {
      setVersionError("Invalid version");
      return;
    }

    if (isEdit) {
      await saveEdit(editedInstall);
      return;
    }
    await createAndInstall(editedInstall);
  };

  const directoryHint = getDirectoryHint(editingInstall.directory);

  return (
    <>
      <Dialog.Title className="dialog-title">{DIALOG_TITLES[dialogType]}</Dialog.Title>

      <div className="dialog-fields">
        <div className="dialog-field">
          <label className="field-label">NAME</label>
          <input
            className="field"
            disabled={editingInstall.is_latest}
            value={editingInstall.name}
            onChange={(event) => changeName(event.target.value)}
            placeholder={DEFAULT_NAME}
            autoFocus
          />
          <span className={`text-[11px] ${nameError ? "text-red" : "text-muted"}`}>
            {nameError}
          </span>
        </div>

        <div className="dialog-field">
          <label className="field-label">VERSION</label>
          <DropdownMenu.Root>
            <DropdownMenu.Trigger asChild>
              <button type="button" className="group field flex items-center justify-between">
                <span>{editingInstall.version}</span>
                <ChevronDown
                  size={ICON_SIZE}
                  className="text-muted transition-transform duration-100 group-data-[state=open]:rotate-180"
                />
              </button>
            </DropdownMenu.Trigger>
            <DropdownMenu.Portal>
              <DropdownMenu.Content
                align="start"
                sideOffset={4}
                className="menu w-(--radix-dropdown-menu-trigger-width)"
              >
                <DropdownMenu.CheckboxItem
                  className="menu-item justify-start gap-2 border-b border-line py-[9px]"
                  checked={showSnapshots}
                  onCheckedChange={toggleSnapshots}
                  onSelect={(event) => event.preventDefault()}
                >
                  <span className="flex size-3 items-center justify-center border border-line-strong">
                    <DropdownMenu.ItemIndicator>
                      <Check size={CHECK_ICON_SIZE} />
                    </DropdownMenu.ItemIndicator>
                  </span>
                  <span>Show snapshots</span>
                </DropdownMenu.CheckboxItem>
                <DropdownMenu.RadioGroup
                  value={editingInstall.version}
                  onValueChange={selectVersion}
                  className="max-h-[200px] overflow-y-auto"
                >
                  {versions.map((version) => {
                    const isRelease = version.version_type === RELEASE_VERSION_TYPE;
                    return (
                      <DropdownMenu.RadioItem
                        key={version.id}
                        value={version.id}
                        className="menu-item"
                      >
                        <span>{version.id}</span>
                        {!isRelease && (
                          <span className="text-[8px] font-semibold tracking-[0.1em] text-faint uppercase">
                            {version.version_type}
                          </span>
                        )}
                      </DropdownMenu.RadioItem>
                    );
                  })}
                </DropdownMenu.RadioGroup>
              </DropdownMenu.Content>
            </DropdownMenu.Portal>
          </DropdownMenu.Root>
          <span className="text-[11px] text-red">{versionError || ""}</span>
        </div>

        <div className="dialog-field">
          <label className="field-label">GAME DIRECTORY</label>
          <div className="flex gap-1.5">
            <input
              className="field min-w-0 flex-1"
              value={editingInstall.directory}
              onChange={(event) => changeDirectory(event.target.value)}
              placeholder={DEFAULT_DIRECTORY}
            />
            <button
              className="button-secondary button-icon h-auto w-9 self-stretch"
              onClick={browseDirectory}
            >
              <Folder size={ICON_SIZE} />
            </button>
          </div>
          <span className={`text-[11px] ${dirError ? "text-red" : "text-muted"}`}>
            {dirError || directoryHint}
          </span>
        </div>

        <div className="dialog-field">
          <label className="field-label">RESOLUTION</label>
          <div className="flex items-center gap-2">
            <input
              type="number"
              className={resolutionInputClass}
              value={editingInstall.width}
              onChange={(event) =>
                setEditingInstall((previous) => ({
                  ...previous,
                  width: parseInt(event.target.value) || DEFAULT_WIDTH,
                }))
              }
              placeholder={String(DEFAULT_WIDTH)}
            />
            <span className="text-sm text-faint">×</span>
            <input
              type="number"
              className={resolutionInputClass}
              value={editingInstall.height}
              onChange={(event) =>
                setEditingInstall((previous) => ({
                  ...previous,
                  height: parseInt(event.target.value) || DEFAULT_HEIGHT,
                }))
              }
              placeholder={String(DEFAULT_HEIGHT)}
            />
          </div>
        </div>
      </div>

      <div className="dialog-actions">
        <button className="button-secondary" onClick={() => setOpenedDialog(null)}>
          Cancel
        </button>
        <button className="button-primary" onClick={save}>
          {SAVE_LABELS[dialogType]}
        </button>
      </div>
    </>
  );
}

const STATUS_CLEAR_DELAY_MS = 3000;
