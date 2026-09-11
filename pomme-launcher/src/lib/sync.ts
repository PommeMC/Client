import { useEffect, useRef } from "react";
import { commands } from "../bindings";
import { getPresenceByUuid, PRESENCE_INTERVAL_MS } from "./friends";
import { PING_INTERVAL_MS } from "./servers";
import { getAccountUuid, useAppStore } from "./store";

const MS_PER_SECOND = 1000;

export function useServerPolling() {
  const loadServers = useAppStore((state) => state.loadServers);
  const pingAllServers = useAppStore((state) => state.pingAllServers);
  const serversLoaded = useAppStore((state) => state.serversLoaded);
  const serverCount = useAppStore((state) => state.servers.length);

  useEffect(() => {
    loadServers();
  }, [loadServers]);

  useEffect(() => {
    const hasServers = serverCount > 0;
    if (!serversLoaded) {
      return;
    }
    if (!hasServers) {
      return;
    }
    const interval = setInterval(pingAllServers, PING_INTERVAL_MS);
    return () => clearInterval(interval);
  }, [serversLoaded, serverCount, pingAllServers]);
}

export function useFriendsSync() {
  const uuid = useAppStore(getAccountUuid);
  const currentActivity = useAppStore((state) => state.currentActivity);
  const presenceRefresh = useAppStore((state) => state.presenceRefresh);
  const resetFriends = useAppStore((state) => state.resetFriends);
  const loadFriends = useAppStore((state) => state.loadFriends);
  const loadFriendSettings = useAppStore((state) => state.loadFriendSettings);
  const setFriendsPresence = useAppStore((state) => state.setFriendsPresence);
  const presenceRequestId = useRef(0);

  useEffect(() => {
    resetFriends();
    const isSignedIn = uuid !== null;
    if (!isSignedIn) {
      return;
    }
    loadFriends(uuid);
    loadFriendSettings(uuid);
  }, [uuid, resetFriends, loadFriends, loadFriendSettings]);

  useEffect(() => {
    const isSignedIn = uuid !== null;
    if (!isSignedIn) {
      return;
    }
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | null = null;

    const sendHeartbeat = async () => {
      presenceRequestId.current += 1;
      const requestId = presenceRequestId.current;
      const result = await commands.updatePresence(
        uuid,
        currentActivity.status,
        currentActivity.joinInfo,
      );
      if (cancelled) {
        return;
      }
      const isStale = requestId !== presenceRequestId.current;
      if (isStale) {
        return;
      }

      let nextDelay = PRESENCE_INTERVAL_MS;
      const succeeded = result.ok;
      if (succeeded) {
        setFriendsPresence(getPresenceByUuid(result.value));
      } else {
        const error = result.error;
        const isRateLimited = error.kind === "rateLimited";
        if (isRateLimited) {
          nextDelay = Math.max(PRESENCE_INTERVAL_MS, error.retryAfterSecs * MS_PER_SECOND);
        }
      }
      timer = setTimeout(sendHeartbeat, nextDelay);
    };

    sendHeartbeat();

    return () => {
      cancelled = true;
      const activeTimer = timer;
      const hasTimer = activeTimer !== null;
      if (hasTimer) {
        clearTimeout(activeTimer);
      }
    };
  }, [uuid, currentActivity, presenceRefresh, setFriendsPresence]);
}
