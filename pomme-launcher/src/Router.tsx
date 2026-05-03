import { getCurrentWindow } from "@tauri-apps/api/window";
import App from "./App";
import Console from "./components/Console";
import { AppStateProvider } from "./lib/state";

export default function Router() {
  if (getCurrentWindow().label === "console") return <Console />;

  return (
    <AppStateProvider>
      <App />
    </AppStateProvider>
  );
}
