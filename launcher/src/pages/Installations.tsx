import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { useEffect } from "react";
import { BiSolidDownload } from "react-icons/bi";
import {
  HiCube,
  HiDocumentDuplicate,
  HiFolder,
  HiPencil,
  HiPlay,
  HiPlus,
  HiTrash,
} from "react-icons/hi2";
import { formatRelativeDate } from "../lib/helpers.ts";
import { useAppStateContext } from "../lib/state";
import type { InstallationError } from "../lib/types.ts";

interface InstallationsPageProps {
  deleteInstallation: (install_id: string) => Promise<null | InstallationError>;
  handleLaunch: () => Promise<void>;
  ensureAssets: (version: string) => Promise<boolean>;
}

export default function InstallationsPage({
  deleteInstallation,
  handleLaunch,
  ensureAssets,
}: InstallationsPageProps) {
  const {
    activeInstall,
    setActiveInstall,
    installations,
    setInstallations,
    setPage,
    setOpenedDialog,
    downloadedVersions,
  } = useAppStateContext();

  useEffect(() => {
    const interval = setInterval(() => {
      setInstallations((prev) => [...prev]);
    }, 60000);

    return () => clearInterval(interval);
  }, [setInstallations]);

  return (
    <div className="page installs-page">
      <div className="installs-header">
        <h2 className="installs-heading">INSTALLATIONS</h2>
        <button
          className="installs-new-btn"
          onClick={() => {
            setOpenedDialog({ name: "installation", props: { type: "new" } });
          }}
        >
          <HiPlus /> New Installation
        </button>
      </div>

      <div className="installs-list">
        {installations.map((inst) => (
          <div
            key={inst.id}
            className={`install-card ${inst.id === activeInstall?.id ? "active" : ""}`}
            onClick={() => {
              setActiveInstall(inst);
            }}
          >
            <div className="install-card-icon">
              <HiCube />
            </div>
            <div className="install-card-info">
              <span className="install-card-name">{inst.name}</span>
              <span className="install-card-version">{inst.version}</span>
            </div>
            <span className="install-card-played">
              {inst.last_played ? formatRelativeDate(inst.last_played) : "Never"}
            </span>
            {downloadedVersions.has(inst.version) ? (
              <button
                className="install-play-btn"
                onClick={() => {
                  setActiveInstall(inst);
                  setPage("home");
                  handleLaunch();
                }}
              >
                <HiPlay /> Play
              </button>
            ) : (
              <button
                className="install-download-btn"
                onClick={() => {
                  setActiveInstall(inst);
                  setPage("home");
                  ensureAssets(inst.version);
                }}
              >
                <BiSolidDownload /> Install
              </button>
            )}
            <button
              className="install-folder-btn"
              onClick={async () => {
                await revealItemInDir(inst.directory);
              }}
            >
              <HiFolder />
            </button>
            <div className="install-card-actions">
              <button
                className="install-action-btn"
                onClick={() => {
                  setOpenedDialog({
                    name: "installation",
                    props: { type: "edit", installation: { ...inst } },
                  });
                }}
                title="Edit"
              >
                <HiPencil />
              </button>

              <button
                className="install-action-btn"
                title="Duplicate"
                onClick={() => {
                  const dup = {
                    ...inst,
                    id: "",
                    name: `${inst.name} (copy)`,
                    directory: `${inst.directory}-copy`,
                    is_latest: false,
                  };
                  setOpenedDialog({
                    name: "installation",
                    props: { type: "dupl", installation: dup, original_id: inst.id },
                  });
                }}
              >
                <HiDocumentDuplicate />
              </button>
              {!inst.is_latest && (
                <button
                  className="install-action-btn delete"
                  title="Delete"
                  onClick={() => {
                    setOpenedDialog({
                      name: "confirm_dialog",
                      props: {
                        title: `Deleting ${inst.name}`,
                        message: "Are you sure you want to delete this installation?",
                        onConfirm: async () => {
                          const index = installations.findIndex((i) => i.id === inst.id);
                          await deleteInstallation(inst.id);
                          setActiveInstall((current) => {
                            if (current?.id !== inst.id) return current;
                            const newList = installations.filter((i) => i.id !== inst.id);
                            return newList[index] ?? newList[index - 1] ?? null;
                          });
                        },
                      },
                    });
                  }}
                >
                  <HiTrash />
                </button>
              )}
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}
