import { StateCreator } from "zustand";
import { commands, Result } from "../bindings";
import {
  Friend,
  FriendSettings,
  FriendsApiError,
  FriendsList,
  PresenceEntry,
  PresenceJoinInfo,
} from "../bindings/pomme_launcher/friends";
import { getAccountUuid } from "./appSlice";
import type { AppStore } from "./store";

const EMPTY_FRIENDS_LIST: FriendsList = { friends: [], incomingRequests: [], outgoingRequests: [] };
export const PRESENCE_INTERVAL_MS = 30_000;
const NO_TIMESTAMP = 0;

export type ActivityStatus = "ONLINE" | "PLAYING_OFFLINE" | "PLAYING_SERVER";
export type Activity = { status: ActivityStatus; joinInfo: PresenceJoinInfo | null };
export const ACTIVITY_IDLE: Activity = { status: "ONLINE", joinInfo: null };

export type FriendsSlice = {
  friendsList: FriendsList;
  friendsSorted: Friend[];
  friendsError: string | null;
  friendsSkins: Record<string, string>;
  friendsPresence: Record<string, PresenceEntry>;
  friendsSettings: FriendSettings | null;
  currentActivity: Activity;
  presenceRefresh: number;
  resetFriends: () => void;
  loadFriends: (uuid: string) => Promise<void>;
  loadFriendSettings: (uuid: string) => Promise<void>;
  setFriendsPresence: (friendsPresence: Record<string, PresenceEntry>) => void;
  sendFriendRequest: (name: string) => Promise<void>;
  acceptFriendRequest: (friendUuid: string) => Promise<void>;
  removeFriend: (friendUuid: string) => Promise<void>;
  updateFriendSettings: (showInList: boolean, acceptInvites: boolean) => Promise<void>;
  refreshPresence: () => void;
  clearFriendsError: () => void;
  setCurrentActivity: (currentActivity: Activity) => void;
};

export function isOffline(presence: PresenceEntry | undefined): boolean {
  const hasPresence = presence !== undefined;
  if (!hasPresence) {
    return true;
  }
  const offline = presence.status === "OFFLINE";
  return offline;
}

function formatFriendsError(error: FriendsApiError): string {
  const isRateLimited = error.kind === "rateLimited";
  if (isRateLimited) {
    const message = `Rate limited, try again in ${error.retryAfterSecs}s`;
    return message;
  }
  const message = error.message;
  return message;
}

function getLastUpdatedTime(presence: PresenceEntry | undefined): number {
  const lastUpdated = presence?.lastUpdated;
  const hasTimestamp = typeof lastUpdated === "string";
  if (!hasTimestamp) {
    return NO_TIMESTAMP;
  }
  const time = Date.parse(lastUpdated);
  return time;
}

function compareFriends(
  first: Friend,
  second: Friend,
  presence: Record<string, PresenceEntry>,
): number {
  const firstPresence = presence[first.profileId];
  const secondPresence = presence[second.profileId];
  const firstOffline = isOffline(firstPresence);
  const secondOffline = isOffline(secondPresence);
  const differentStatus = firstOffline !== secondOffline;
  if (differentStatus) {
    const offlineLast = firstOffline ? 1 : -1;
    return offlineLast;
  }
  const newestFirst = getLastUpdatedTime(secondPresence) - getLastUpdatedTime(firstPresence);
  return newestFirst;
}

function getSortedFriends(
  friendsList: FriendsList,
  presence: Record<string, PresenceEntry>,
): Friend[] {
  const friends = friendsList.friends ?? [];
  const sorted = [...friends].sort((first, second) => compareFriends(first, second, presence));
  return sorted;
}

function getAllListedFriends(friendsList: FriendsList): Friend[] {
  const listed = [
    ...(friendsList.friends ?? []),
    ...(friendsList.incomingRequests ?? []),
    ...(friendsList.outgoingRequests ?? []),
  ];
  return listed;
}

export function getPresenceByUuid(entries: PresenceEntry[]): Record<string, PresenceEntry> {
  const byUuid: Record<string, PresenceEntry> = {};
  for (const entry of entries) {
    byUuid[entry.profileId] = entry;
  }
  return byUuid;
}

export const createFriendsSlice: StateCreator<AppStore, [], [], FriendsSlice> = (set, get) => {
  const loadFriendSkin = async (friendUuid: string) => {
    const alreadyLoaded = friendUuid in get().friendsSkins;
    if (alreadyLoaded) {
      return;
    }
    const result = await commands.getSkinUrl(friendUuid);
    const succeeded = result.ok;
    if (!succeeded) {
      return;
    }
    set({ friendsSkins: { ...get().friendsSkins, [friendUuid]: result.value } });
  };

  const applyFriendsList = (friendsList: FriendsList) => {
    const friendsSorted = getSortedFriends(friendsList, get().friendsPresence);
    set({ friendsList, friendsSorted });
    for (const friend of getAllListedFriends(friendsList)) {
      loadFriendSkin(friend.profileId);
    }
  };

  const runFriendMutation = async <T>(
    operation: Promise<Result<T, FriendsApiError>>,
    onSuccess: (value: T) => void,
  ) => {
    const result = await operation;
    const succeeded = result.ok;
    if (!succeeded) {
      set({ friendsError: formatFriendsError(result.error) });
      return;
    }
    onSuccess(result.value);
    set({ friendsError: null });
  };

  return {
    friendsList: EMPTY_FRIENDS_LIST,
    friendsSorted: [],
    friendsError: null,
    friendsSkins: {},
    friendsPresence: {},
    friendsSettings: null,
    currentActivity: ACTIVITY_IDLE,
    presenceRefresh: 0,

    resetFriends: () => {
      set({
        friendsList: EMPTY_FRIENDS_LIST,
        friendsSorted: [],
        friendsError: null,
        friendsSkins: {},
        friendsPresence: {},
        friendsSettings: null,
      });
    },

    loadFriends: async (uuid) => {
      const result = await commands.getFriends(uuid);
      const isStale = getAccountUuid(get()) !== uuid;
      if (isStale) {
        return;
      }
      const succeeded = result.ok;
      if (!succeeded) {
        set({ friendsError: formatFriendsError(result.error) });
        return;
      }
      applyFriendsList(result.value);
      set({ friendsError: null });
    },

    loadFriendSettings: async (uuid) => {
      const result = await commands.getFriendSettings(uuid);
      const isStale = getAccountUuid(get()) !== uuid;
      if (isStale) {
        return;
      }
      const succeeded = result.ok;
      if (!succeeded) {
        return;
      }
      set({ friendsSettings: result.value });
    },

    setFriendsPresence: (friendsPresence) => {
      const friendsSorted = getSortedFriends(get().friendsList, friendsPresence);
      set({ friendsPresence, friendsSorted });
    },

    sendFriendRequest: async (name) => {
      const uuid = getAccountUuid(get());
      const isSignedIn = uuid !== null;
      if (!isSignedIn) {
        return;
      }
      await runFriendMutation(commands.sendFriendRequest(uuid, name), applyFriendsList);
    },

    acceptFriendRequest: async (friendUuid) => {
      const uuid = getAccountUuid(get());
      const isSignedIn = uuid !== null;
      if (!isSignedIn) {
        return;
      }
      await runFriendMutation(commands.acceptFriendRequest(uuid, friendUuid), applyFriendsList);
    },

    removeFriend: async (friendUuid) => {
      const uuid = getAccountUuid(get());
      const isSignedIn = uuid !== null;
      if (!isSignedIn) {
        return;
      }
      await runFriendMutation(commands.removeFriend(uuid, friendUuid), applyFriendsList);
    },

    updateFriendSettings: async (showInList, acceptInvites) => {
      const uuid = getAccountUuid(get());
      const isSignedIn = uuid !== null;
      if (!isSignedIn) {
        return;
      }
      await runFriendMutation(
        commands.updateFriendSettings(uuid, showInList, acceptInvites),
        (friendsSettings) => set({ friendsSettings }),
      );
    },

    refreshPresence: () => set({ presenceRefresh: get().presenceRefresh + 1 }),
    clearFriendsError: () => set({ friendsError: null }),
    setCurrentActivity: (currentActivity) => set({ currentActivity }),
  };
};

export type { Friend, FriendSettings, PresenceEntry };

const SECONDS_PER_MINUTE = 60;
const SECONDS_PER_HOUR = 3600;
const SECONDS_PER_DAY = 86_400;
const SECONDS_PER_WEEK = 604_800;
const MS_PER_SECOND = 1000;
const STATUS_TEXT: Record<string, string> = {
  ONLINE: "Online",
  PLAYING_OFFLINE: "In singleplayer",
  PLAYING_REALMS: "Playing Realms",
  PLAYING_HOSTED_SERVER: "Hosting local world",
};

function formatLastSeen(iso: string | null | undefined): string {
  const hasTimestamp = typeof iso === "string";
  if (!hasTimestamp) {
    return "";
  }
  const then = Date.parse(iso);
  const isInvalid = Number.isNaN(then);
  if (isInvalid) {
    return "";
  }
  const deltaSeconds = Math.max(0, (Date.now() - then) / MS_PER_SECOND);
  const isJustNow = deltaSeconds < SECONDS_PER_MINUTE;
  if (isJustNow) {
    const justNow = "just now";
    return justNow;
  }
  const isMinutes = deltaSeconds < SECONDS_PER_HOUR;
  if (isMinutes) {
    const minutesAgo = `${Math.floor(deltaSeconds / SECONDS_PER_MINUTE)}m ago`;
    return minutesAgo;
  }
  const isHours = deltaSeconds < SECONDS_PER_DAY;
  if (isHours) {
    const hoursAgo = `${Math.floor(deltaSeconds / SECONDS_PER_HOUR)}h ago`;
    return hoursAgo;
  }
  const isDays = deltaSeconds < SECONDS_PER_WEEK;
  if (isDays) {
    const daysAgo = `${Math.floor(deltaSeconds / SECONDS_PER_DAY)}d ago`;
    return daysAgo;
  }
  const date = new Date(then).toLocaleDateString(undefined, { month: "short", day: "numeric" });
  return date;
}

function formatOfflineText(lastUpdated: string | null | undefined): string {
  const seen = formatLastSeen(lastUpdated);
  const hasSeen = seen !== "";
  const offlineText = hasSeen ? `Offline · ${seen}` : "Offline";
  return offlineText;
}

export function formatStatus(presence: PresenceEntry | undefined): string {
  const hasPresence = presence !== undefined;
  if (!hasPresence) {
    const unknownText = formatOfflineText(undefined);
    return unknownText;
  }
  const isOfflineStatus = presence.status === "OFFLINE";
  if (isOfflineStatus) {
    const offlineText = formatOfflineText(presence.lastUpdated);
    return offlineText;
  }
  const isOnServer = presence.status === "PLAYING_SERVER";
  if (isOnServer) {
    const address = presence.joinInfo?.value;
    const hasAddress = address !== undefined;
    const serverText = hasAddress ? `Playing: ${address}` : "Playing multiplayer";
    return serverText;
  }
  const statusText = STATUS_TEXT[presence.status] ?? presence.status;
  return statusText;
}
