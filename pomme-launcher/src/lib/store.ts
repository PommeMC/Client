import { create } from "zustand";
import { AppSlice, createAppSlice } from "./appSlice";
import { createFriendsSlice, FriendsSlice } from "./friends";
import { createLauncherSettingsSlice, LauncherSettingsSlice } from "./launcherSettings";
import { createServersSlice, ServersSlice } from "./servers";

export type AppStore = AppSlice & LauncherSettingsSlice & ServersSlice & FriendsSlice;

export const useAppStore = create<AppStore>()((...storeArgs) => ({
  ...createAppSlice(...storeArgs),
  ...createLauncherSettingsSlice(...storeArgs),
  ...createServersSlice(...storeArgs),
  ...createFriendsSlice(...storeArgs),
}));

export { getAccount, getAccountUuid } from "./appSlice";
