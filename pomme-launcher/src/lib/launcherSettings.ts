import { StateCreator } from "zustand";
import { commands } from "../bindings";
import { LauncherSettings } from "../bindings/pomme_launcher/settings";
import type { AppStore } from "./store";

const DEFAULT_LAUNCHER_SETTINGS: LauncherSettings = {
  language: "English",
  keepLauncherOpen: true,
  launchWithConsole: false,
};

export type LauncherSettingsSlice = {
  launcherSettings: LauncherSettings;
  loadLauncherSettings: () => Promise<void>;
  setLanguage: (language: string) => Promise<void>;
  setKeepLauncherOpen: (keepLauncherOpen: boolean) => Promise<void>;
  setLaunchWithConsole: (launchWithConsole: boolean) => Promise<void>;
};

export const createLauncherSettingsSlice: StateCreator<AppStore, [], [], LauncherSettingsSlice> = (
  set,
  get,
) => ({
  launcherSettings: DEFAULT_LAUNCHER_SETTINGS,

  loadLauncherSettings: async () => {
    try {
      const launcherSettings = await commands.loadLauncherSettings();
      set({ launcherSettings });
    } catch (error) {
      console.error("Failed to load launcher settings:", error);
    }
  },

  setLanguage: async (language) => {
    const result = await commands.setLauncherLanguage(language);
    const succeeded = result.ok;
    if (!succeeded) {
      console.error("Failed to set launcher language:", result.error);
      return;
    }
    set({ launcherSettings: { ...get().launcherSettings, language } });
  },

  setKeepLauncherOpen: async (keepLauncherOpen) => {
    const result = await commands.setKeepLauncherOpen(keepLauncherOpen);
    const succeeded = result.ok;
    if (!succeeded) {
      console.error("Failed to set keep launcher open:", result.error);
      return;
    }
    set({ launcherSettings: { ...get().launcherSettings, keepLauncherOpen } });
  },

  setLaunchWithConsole: async (launchWithConsole) => {
    const result = await commands.setLaunchWithConsole(launchWithConsole);
    const succeeded = result.ok;
    if (!succeeded) {
      console.error("Failed to set launch with console:", result.error);
      return;
    }
    set({ launcherSettings: { ...get().launcherSettings, launchWithConsole } });
  },
});
