import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { Box, Copy, Download, Folder, Pencil, Play, Plus, Trash2 } from "lucide-react";
import { useEffect, useState } from "react";
import { commands } from "../bindings";
import { Installation } from "../bindings/pomme_launcher/installations";
import { formatRelativeDate } from "../lib/helpers";
import { useAppStore } from "../lib/store";
import type { handleLaunchType } from "../lib/types";

const RELATIVE_DATE_REFRESH_MS = 60_000;
const ICON_SIZE = 14;
const BUTTON_ICON_SIZE = 12;
const CARD_ICON_SIZE = 16;

interface InstallationsPageProps {
  handleLaunch: handleLaunchType;
}

function getLastPlayedText(installation: Installation): string {
  const lastPlayed = installation.last_played;
  const hasPlayed = lastPlayed !== null;
  if (!hasPlayed) {
    return "Never";
  }
  const relative = formatRelativeDate(lastPlayed);
  return relative;
}

export default function InstallationsPage({ handleLaunch }: InstallationsPageProps) {
  const activeInstall = useAppStore((state) => state.activeInstall);
  const setActiveInstall = useAppStore((state) => state.setActiveInstall);
  const installations = useAppStore((state) => state.installations);
  const removeInstallation = useAppStore((state) => state.removeInstallation);
  const setPage = useAppStore((state) => state.setPage);
  const setOpenedDialog = useAppStore((state) => state.setOpenedDialog);
  const downloadedVersions = useAppStore((state) => state.downloadedVersions);

  const [, setRenderTick] = useState(0);
  useEffect(() => {
    const interval = setInterval(() => {
      setRenderTick((tick) => tick + 1);
    }, RELATIVE_DATE_REFRESH_MS);
    return () => clearInterval(interval);
  }, []);

  const openNewDialog = () => {
    setOpenedDialog({ name: "installation_dialog", props: { type: "new" } });
  };

  const play = (installation: Installation) => {
    setActiveInstall(installation);
    setPage("home");
    handleLaunch({ install: installation });
  };

  const openEditDialog = (installation: Installation) => {
    setOpenedDialog({
      name: "installation_dialog",
      props: { type: "edit", installation: { ...installation } },
    });
  };

  const openDuplicateDialog = (installation: Installation) => {
    const duplicate: Installation = {
      ...installation,
      id: "",
      name: `${installation.name} (copy)`,
      directory: `${installation.directory}-copy`,
      is_latest: false,
    };
    setOpenedDialog({
      name: "installation_dialog",
      props: { type: "dupl", installation: duplicate, original_id: installation.id },
    });
  };

  const confirmDelete = (installation: Installation) => {
    setOpenedDialog({
      name: "confirm_dialog",
      props: {
        title: `Deleting ${installation.name}`,
        message: "Are you sure you want to delete this installation?",
        onConfirm: async () => {
          const result = await commands.deleteInstallation(installation.id);
          const succeeded = result.ok;
          let canRemove = succeeded;
          if (!succeeded) {
            const alreadyMissing = result.error.kind === "InstallNotFound";
            canRemove = alreadyMissing;
          }
          if (!canRemove) {
            return;
          }
          removeInstallation(installation.id);
        },
      },
    });
  };

  return (
    <div className="page">
      <div className="page-header">
        <h2 className="page-heading">INSTALLATIONS</h2>
        <button className="button-primary" onClick={openNewDialog}>
          <Plus size={ICON_SIZE} /> New Installation
        </button>
      </div>

      <div className="list">
        {installations.map((installation) => {
          const isActive = installation.id === activeInstall?.id;
          const isDownloaded = downloadedVersions.has(installation.version);
          return (
            <div
              key={installation.id}
              className={`row gap-3.5 border-l-2 py-3.5 pr-4 pl-3.5 ${
                isActive ? "border-l-green" : "border-l-transparent"
              }`}
              onClick={() => setActiveInstall(installation)}
            >
              <Box
                size={CARD_ICON_SIZE}
                className={isActive ? "shrink-0 text-green" : "shrink-0 text-faint"}
              />
              <div className="flex min-w-0 flex-1 flex-col gap-0.5">
                <span className="text-sm font-medium text-foreground">{installation.name}</span>
                <span className="text-xs text-muted tabular-nums">{installation.version}</span>
              </div>
              <span className="mr-1 shrink-0 text-xs text-faint">
                {getLastPlayedText(installation)}
              </span>
              <button className="button-primary" onClick={() => play(installation)}>
                {isDownloaded ? (
                  <>
                    <Play size={BUTTON_ICON_SIZE} fill="currentColor" /> Play
                  </>
                ) : (
                  <>
                    <Download size={BUTTON_ICON_SIZE} /> Install
                  </>
                )}
              </button>
              <button
                className="button-secondary button-icon"
                onClick={() => revealItemInDir(installation.directory)}
              >
                <Folder size={ICON_SIZE} />
              </button>
              <div className="ml-1.5 flex shrink-0 gap-1 border-l border-white/[0.08] pl-2.5">
                <button
                  className="button-secondary button-icon"
                  onClick={() => openEditDialog(installation)}
                  title="Edit"
                >
                  <Pencil size={ICON_SIZE} />
                </button>
                <button
                  className="button-secondary button-icon"
                  title="Duplicate"
                  onClick={() => openDuplicateDialog(installation)}
                >
                  <Copy size={ICON_SIZE} />
                </button>
                {!installation.is_latest && (
                  <button
                    className="button-secondary button-icon hover:enabled:border-red/50 hover:enabled:text-red"
                    title="Delete"
                    onClick={() => confirmDelete(installation)}
                  >
                    <Trash2 size={ICON_SIZE} />
                  </button>
                )}
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
}
