import {
  HiPlay,
  HiCube,
  HiDocumentDuplicate,
  HiFolder,
  HiPencil,
  HiPlus,
  HiTrash,
} from "react-icons/hi2";
import { useAppStateContext } from "../lib/state";
import { useEffect } from "react";
import { formatRelativeDate } from "../lib/helpers.ts";

interface InstallationsPageProps {
  deleteInstallation: (install_id: string) => Promise<void>;
}

export default function InstallationsPage({ deleteInstallation }: InstallationsPageProps) {
  const {
    activeInstall,
    setActiveInstall,
    installations,
    setInstallations,
    setPage,
    setOpenedDialog,
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
            setOpenedDialog({ name: "installation", props: { editing: false } });
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
          >
            <div className="install-card-icon">
              <HiCube />
            </div>
            <div className="install-card-info">
              <span className="install-card-name">{inst.name}</span>
              <span className="install-card-version">{inst.version}</span>
            </div>
            <span className="install-card-played">
              {inst.lastPlayed ? formatRelativeDate(inst.lastPlayed) : "Never"}
            </span>
            <button
              className="install-play-btn"
              onClick={() => {
                setActiveInstall(inst);
                setPage("home");
              }}
            >
              <HiPlay /> Play
            </button>
            <button
              className="install-folder-btn"
              onClick={() => console.log("Open:", inst.directory)}
            >
              <HiFolder />
            </button>
            <div className="install-card-actions">
              <button
                className="install-action-btn"
                onClick={() => {
                  setOpenedDialog({
                    name: "installation",
                    props: { editing: true, installation: { ...inst } },
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
                  };
                  setOpenedDialog({
                    name: "installation",
                    props: { editing: true, installation: dup },
                  });
                }}
              >
                <HiDocumentDuplicate />
              </button>
              {!inst.isLatest && (
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
                          await deleteInstallation(inst.id);
                          setInstallations((prev) => {
                            const index = prev.findIndex((x) => x.id === inst.id);
                            const newList = prev.filter((i) => i.id !== inst.id);
                            setActiveInstall((current) => {
                              if (current?.id !== inst.id) return current;
                              return newList[index] || newList[index - 1] || null;
                            });
                            return newList;
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
