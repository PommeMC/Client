import { BiSolidDownload } from "react-icons/bi";
import { HiChevronDown, HiCube, HiPlay } from "react-icons/hi2";
import SkinRunner from "../components/SkinRunner";
import { useDropdown } from "../lib/hooks";
import { useAppStateContext } from "../lib/state";
import { PatchNote } from "../lib/types";

interface HomepageProps {
  handleLaunch: () => Promise<void>;
  openPatchNote: (item: PatchNote) => Promise<void>;
}

export default function Homepage({ handleLaunch, openPatchNote }: HomepageProps) {
  const {
    launchingStatus,
    installations,
    activeInstall,
    setActiveInstall,
    news,
    status,
    downloadedVersions,
    downloadProgress,
    skinUrl,
    setOpenedDialog,
  } = useAppStateContext();

  const { ref: versionDropdownRef, ...versionDropdown } = useDropdown();

  return (
    <div className="page home-page">
      <div className="hero-banner">
        <div className="hero-overlay" />
        <div className="hero-content">
          <h1 className="hero-title">POMME</h1>
          <p className="hero-subtitle">RUST-NATIVE MINECRAFT CLIENT</p>
        </div>
      </div>

      <div className="launch-bar">
        <button
          className={`play-button ${
            launchingStatus === "installing" || launchingStatus === "checking_assets"
              ? "installing"
              : launchingStatus === "launching"
                ? "launching"
                : ""
          }`}
          onClick={handleLaunch}
          disabled={launchingStatus !== null}
        >
          {launchingStatus === null && downloadedVersions.has(activeInstall?.version ?? "") ? (
            <HiPlay className="play-icon" />
          ) : (
            <BiSolidDownload className="download-icon" />
          )}
          <span className="play-text">
            {launchingStatus === null
              ? downloadedVersions.has(activeInstall?.version ?? "")
                ? "PLAY"
                : "INSTALL"
              : launchingStatus === "checking_assets"
                ? "Checking assets..."
                : launchingStatus === "installing"
                  ? "Installing..."
                  : "Launching..."}
          </span>
        </button>
      </div>

      <div className="version-badge-wrapper" ref={versionDropdownRef}>
        <button className="version-badge" onClick={versionDropdown.toggle}>
          <HiCube className="version-badge-icon" />
          <span className="version-item-id">
            {activeInstall?.name || "No installation selected"}
          </span>
          <span className="version-item-type" hidden={!activeInstall}>
            {activeInstall?.version || ""}
          </span>
          <HiChevronDown
            className={`version-badge-arrow ${versionDropdown.isOpen ? "open" : ""}`}
          />
        </button>
        {versionDropdown.isOpen && (
          <div className="version-dropdown">
            <div className="version-list">
              {installations.length === 0 ? (
                <button
                  className={`version-item`}
                  onClick={() => {
                    versionDropdown.close();
                    setOpenedDialog({ name: "installation", props: { type: "new" } });
                  }}
                >
                  <span className="version-item-id">Create a new installation</span>
                </button>
              ) : (
                installations.map((inst) => (
                  <button
                    key={inst.id}
                    className={`version-item ${inst.id === activeInstall?.id ? "active" : ""}`}
                    onClick={() => {
                      setActiveInstall(inst);
                      versionDropdown.close();
                    }}
                  >
                    <span className="version-item-id">{inst.name}</span>
                    <span className="version-item-type">{inst.version}</span>
                  </button>
                ))
              )}
            </div>
          </div>
        )}
      </div>

      {downloadProgress && (
        <div className="download-progress">
          <div className="download-progress-text">{downloadProgress.status}</div>
          <div className="download-progress-bar">
            <SkinRunner
              skinUrl={skinUrl}
              progress={
                downloadProgress.total > 0
                  ? downloadProgress.downloaded / downloadProgress.total
                  : 0
              }
            />
            <div
              className="download-progress-fill"
              style={{
                width:
                  downloadProgress.total > 0
                    ? `${(downloadProgress.downloaded / downloadProgress.total) * 100}%`
                    : "0%",
              }}
            />
          </div>
        </div>
      )}
      {!downloadProgress && status && <div className="status-toast">{status}</div>}

      <div className="news-section">
        <h2 className="news-heading">LATEST NEWS</h2>
        <div className="news-grid">
          {news.slice(0, 3).map((item) => (
            <div className="news-card" key={item.version} onClick={() => openPatchNote(item)}>
              <div className="news-card-img">
                <img src={item.image_url} alt={item.title} className="news-card-img-bg" />
                <span className="news-type-badge">{item.entry_type}</span>
              </div>

              <div className="news-card-body">
                <div className="news-card-meta">
                  <span className="news-date">{item.date.replace(/-/g, ".")}</span>
                  <span className="news-card-arrow">→</span>
                </div>

                <h3 className="news-title">{item.title}</h3>
                <hr className="news-rule" />
                <p className="news-desc">{item.summary}</p>
              </div>
            </div>
          ))}
          {news.length === 0 && <p className="news-loading">Loading patch notes...</p>}
        </div>
      </div>
    </div>
  );
}
