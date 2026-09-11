import { getCurrentWindow } from "@tauri-apps/api/window";
import App from "./App";
import Console from "./components/Console";

const CONSOLE_WINDOW_LABEL = "console";

export default function Router() {
  const isConsoleWindow = getCurrentWindow().label === CONSOLE_WINDOW_LABEL;
  if (isConsoleWindow) {
    return <Console />;
  }
  return <App />;
}
