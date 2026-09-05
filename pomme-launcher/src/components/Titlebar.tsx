import { getCurrentWindow } from "@tauri-apps/api/window";
import { Minus, Square, X } from "lucide-react";

const ICON_SIZE = 14;
const MAXIMIZE_ICON_SIZE = 12;
const controlButtonClass =
  "flex h-7 w-9 items-center justify-center rounded-md text-muted transition-colors duration-150 hover:text-foreground";

export default function Titlebar({ name }: { name?: string } = { name: "Pomme Launcher" }) {
  const appWindow = getCurrentWindow();

  const minimize = () => {
    appWindow.minimize();
  };
  const toggleMaximize = () => {
    appWindow.toggleMaximize();
  };
  const close = () => {
    appWindow.close();
  };

  return (
    <div
      className="flex h-9 shrink-0 items-center justify-between border-b border-white/[0.06] bg-panel/75 backdrop-blur-xl select-none"
      data-tauri-drag-region
    >
      <div className="w-[108px]" data-tauri-drag-region />
      <span className="text-xs text-muted" data-tauri-drag-region>
        {name}
      </span>
      <div className="flex w-[108px] items-center justify-end gap-0.5 pr-1.5">
        <button className={`${controlButtonClass} hover:bg-white/[0.06]`} onClick={minimize}>
          <Minus size={ICON_SIZE} />
        </button>
        <button className={`${controlButtonClass} hover:bg-white/[0.06]`} onClick={toggleMaximize}>
          <Square size={MAXIMIZE_ICON_SIZE} />
        </button>
        <button className={`${controlButtonClass} hover:bg-red hover:text-white`} onClick={close}>
          <X size={ICON_SIZE} />
        </button>
      </div>
    </div>
  );
}
