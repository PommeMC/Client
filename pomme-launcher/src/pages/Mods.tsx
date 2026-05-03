import { HiListBullet, HiMagnifyingGlass, HiPuzzlePiece, HiSquares2X2 } from "react-icons/hi2";
import { useAppStateContext } from "../lib/state";

export default function ModsPage() {
  const { modFilter, modSearch, setModSearch, setModFilter, modView, setModView } =
    useAppStateContext();

  const mods = [
    {
      name: "Mod 1",
      cat: "performance",
      desc: "Rendering engine optimization for better frame rates",
      version: "0.6.1",
      downloads: "38M",
      installed: true,
    },
    {
      name: "Mod 2",
      cat: "performance",
      desc: "Dynamic lighting and visual enhancement",
      version: "1.21.11",
      downloads: "142M",
      installed: false,
    },
    {
      name: "Mod 3",
      cat: "shaders",
      desc: "Shader pack loader for post-processing effects",
      version: "1.8.0",
      downloads: "25M",
      installed: false,
    },
    {
      name: "Mod 4",
      cat: "utility",
      desc: "Schematic building tools for pasting and moving structures",
      version: "0.19.0",
      downloads: "18M",
      installed: false,
    },
    {
      name: "Mod 5",
      cat: "utility",
      desc: "Real-time mapping with waypoints and minimap",
      version: "6.0.0",
      downloads: "52M",
      installed: true,
    },
    {
      name: "Mod 6",
      cat: "gameplay",
      desc: "Adds new biomes, creatures, and world generation",
      version: "2.3.0",
      downloads: "12M",
      installed: false,
    },
    {
      name: "Mod 7",
      cat: "utility",
      desc: "Inventory sorting and management tools",
      version: "1.4.2",
      downloads: "8M",
      installed: false,
    },
    {
      name: "Mod 8",
      cat: "shaders",
      desc: "Volumetric clouds and atmospheric effects",
      version: "3.1.0",
      downloads: "15M",
      installed: false,
    },
  ];
  const filtered = mods.filter(
    (m) =>
      (modFilter === "all" || m.cat === modFilter) &&
      m.name.toLowerCase().includes(modSearch.toLowerCase()),
  );
  return (
    <div className="page mock-page">
      <div className="mock-banner">This is a preview - functionality coming soon</div>
      <h2 className="page-heading">MODS</h2>
      <div className="mods-toolbar">
        <div className="mods-search">
          <HiMagnifyingGlass className="mods-search-icon" />
          <input
            className="mods-search-input"
            placeholder="Search mods..."
            value={modSearch}
            onChange={(e) => setModSearch(e.target.value)}
          />
        </div>
        <div className="mods-filters">
          {["all", "performance", "shaders", "utility", "gameplay"].map((f) => (
            <button
              key={f}
              className={`mods-filter ${modFilter === f ? "active" : ""}`}
              onClick={() => setModFilter(f)}
            >
              {f.charAt(0).toUpperCase() + f.slice(1)}
            </button>
          ))}
        </div>
        <div className="mods-view-toggle">
          <button
            className={`mods-view-btn ${modView === "list" ? "active" : ""}`}
            onClick={() => setModView("list")}
          >
            <HiListBullet />
          </button>
          <button
            className={`mods-view-btn ${modView === "grid" ? "active" : ""}`}
            onClick={() => setModView("grid")}
          >
            <HiSquares2X2 />
          </button>
        </div>
      </div>
      <div className={modView === "grid" ? "mods-grid" : "mock-list"}>
        {filtered.map((m) => (
          <div className={modView === "grid" ? "mock-mod-card" : "mock-mod"} key={m.name}>
            <div className="mock-mod-icon">
              <HiPuzzlePiece />
            </div>
            <div className="mock-mod-info">
              <span className="mock-mod-name">{m.name}</span>
              <span className="mock-mod-desc">{m.desc}</span>
              <div className="mock-mod-meta">
                <span>{m.version}</span>
                <span>{m.downloads} downloads</span>
              </div>
            </div>
            <button className={`mock-mod-btn ${m.installed ? "installed" : ""}`}>
              {m.installed ? "Installed" : "Install"}
            </button>
          </div>
        ))}
      </div>
    </div>
  );
}
