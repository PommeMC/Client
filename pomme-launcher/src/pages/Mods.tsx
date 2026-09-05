import { LayoutGrid, List, Puzzle, Search } from "lucide-react";
import { MOD_FILTER_ALL } from "../lib/appSlice";
import { useAppStore } from "../lib/store";

const ICON_SIZE = 14;
const LIST_MOD_ICON_SIZE = 16;
const GRID_MOD_ICON_SIZE = 20;
const MOD_FILTERS = [MOD_FILTER_ALL, "performance", "shaders", "utility", "gameplay"];

interface Mod {
  name: string;
  cat: string;
  desc: string;
  version: string;
  downloads: string;
  installed: boolean;
}

const MODS: Mod[] = [
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

function capitalize(word: string): string {
  const capitalized = word.charAt(0).toUpperCase() + word.slice(1);
  return capitalized;
}

function isModVisible(mod: Mod, filter: string, search: string): boolean {
  const showsAll = filter === MOD_FILTER_ALL;
  const matchesFilter = showsAll || mod.cat === filter;
  const matchesSearch = mod.name.toLowerCase().includes(search.toLowerCase());
  const visible = matchesFilter && matchesSearch;
  return visible;
}

export default function ModsPage() {
  const modFilter = useAppStore((state) => state.modFilter);
  const modSearch = useAppStore((state) => state.modSearch);
  const setModSearch = useAppStore((state) => state.setModSearch);
  const setModFilter = useAppStore((state) => state.setModFilter);
  const modView = useAppStore((state) => state.modView);
  const setModView = useAppStore((state) => state.setModView);

  const isGrid = modView === "grid";
  const visibleMods = MODS.filter((mod) => isModVisible(mod, modFilter, modSearch));

  return (
    <div className="page">
      <div className="mb-5 rounded-xl border border-white/[0.08] bg-black/25 px-4 py-3 text-xs font-medium text-muted shadow-[inset_0_1px_0_rgba(255,255,255,0.06)] backdrop-blur-xl">
        This is a preview - functionality coming soon
      </div>
      <h2 className="page-heading mb-6">MODS</h2>

      <div className="mb-5 flex flex-wrap items-center gap-2">
        <div className="flex h-9 min-w-[180px] flex-1 items-center gap-2 rounded-lg border border-white/[0.08] bg-black/30 px-3 shadow-[inset_0_1px_0_rgba(255,255,255,0.06)] transition-colors duration-150 focus-within:border-green/60 hover:border-white/15">
          <Search size={ICON_SIZE} className="shrink-0 text-muted" />
          <input
            className="w-full bg-transparent text-[13px] text-foreground placeholder:text-faint"
            placeholder="Search mods..."
            value={modSearch}
            onChange={(event) => setModSearch(event.target.value)}
          />
        </div>
        <div className="flex gap-1">
          {MOD_FILTERS.map((filter) => {
            const isActive = modFilter === filter;
            return (
              <button
                key={filter}
                className={`h-9 rounded-lg border px-3 text-xs font-medium transition-colors duration-150 ${
                  isActive
                    ? "border-green text-green"
                    : "border-white/[0.08] bg-black/25 text-muted hover:border-white/15 hover:text-foreground"
                }`}
                onClick={() => setModFilter(filter)}
              >
                {capitalize(filter)}
              </button>
            );
          })}
        </div>
        <div className="flex overflow-hidden rounded-lg border border-white/[0.08] bg-black/25">
          <button
            className={`flex h-9 w-9 items-center justify-center transition-colors duration-150 hover:text-foreground ${
              isGrid ? "text-faint" : "bg-white/[0.08] text-foreground"
            }`}
            onClick={() => setModView("list")}
          >
            <List size={ICON_SIZE} />
          </button>
          <button
            className={`flex h-9 w-9 items-center justify-center border-l border-white/[0.08] transition-colors duration-150 hover:text-foreground ${
              isGrid ? "bg-white/[0.08] text-foreground" : "text-faint"
            }`}
            onClick={() => setModView("grid")}
          >
            <LayoutGrid size={ICON_SIZE} />
          </button>
        </div>
      </div>

      <div
        className={isGrid ? "grid grid-cols-[repeat(auto-fill,minmax(240px,1fr))] gap-3" : "list"}
      >
        {visibleMods.map((mod) => (
          <div
            className={
              isGrid
                ? "flex flex-col gap-2.5 rounded-xl border border-white/[0.08] bg-black/25 p-4 shadow-[inset_0_1px_0_rgba(255,255,255,0.06)] backdrop-blur-xl transition-colors duration-150 hover:border-white/15"
                : "row gap-3.5 p-3.5"
            }
            key={mod.name}
          >
            <Puzzle
              size={isGrid ? GRID_MOD_ICON_SIZE : LIST_MOD_ICON_SIZE}
              className="shrink-0 text-faint"
            />
            <div className="flex min-w-0 flex-1 flex-col gap-1">
              <span className="text-sm font-medium text-foreground">{mod.name}</span>
              <span className="text-xs leading-snug text-muted">{mod.desc}</span>
              <div className="flex gap-3 text-[11px] text-faint tabular-nums">
                <span>{mod.version}</span>
                <span>{mod.downloads} downloads</span>
              </div>
            </div>
            <button
              className={`${mod.installed ? "button-secondary cursor-default" : "button-primary"} ${
                isGrid ? "mt-auto w-full" : ""
              }`}
            >
              {mod.installed ? "Installed" : "Install"}
            </button>
          </div>
        ))}
      </div>
    </div>
  );
}
