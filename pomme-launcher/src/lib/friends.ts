import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { commands } from "../bindings";
import {
  Friend,
  FriendSettings,
  FriendsApiError,
  FriendsList,
  PresenceEntry,
  PresenceJoinInfo,
} from "../bindings/pomme_launcher/friends";

const EMPTY: FriendsList = { friends: [], incomingRequests: [], outgoingRequests: [] };
const PRESENCE_INTERVAL_MS = 30_000;

export type ActivityStatus = "ONLINE" | "PLAYING_OFFLINE" | "PLAYING_SERVER";
export type Activity = { status: ActivityStatus; joinInfo: PresenceJoinInfo | null };
export const ACTIVITY_IDLE: Activity = { status: "ONLINE", joinInfo: null };

export const isOffline = (p: PresenceEntry | undefined): boolean => !p || p.status === "OFFLINE";

const formatError = (err: FriendsApiError): string =>
  err.kind === "rateLimited" ? `Rate limited, try again in ${err.retryAfterSecs}s` : err.message;

export const useFriends = (uuid: string | null) => {
  const [friendsList, setFriendsList] = useState<FriendsList>(EMPTY);
  const [friendsError, setFriendsError] = useState<string | null>(null);
  const [friendsSkins, setFriendsSkins] = useState<Record<string, string>>({});
  const [friendsPresence, setFriendsPresence] = useState<Record<string, PresenceEntry>>({});
  const [friendsSettings, setFriendsSettings] = useState<FriendSettings | null>(null);
  const [currentActivity, setCurrentActivity] = useState<Activity>(ACTIVITY_IDLE);
  const [prevUuid, setPrevUuid] = useState(uuid);
  const [presenceRefresh, setPresenceRefresh] = useState(0);
  const presenceReqId = useRef(0);
  const presenceTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  if (uuid !== prevUuid) {
    setPrevUuid(uuid);
    setFriendsList(EMPTY);
    setFriendsSkins({});
    setFriendsPresence({});
    setFriendsSettings(null);
    setFriendsError(null);
  }

  const loadSkinFor = useCallback((friendUuid: string) => {
    setFriendsSkins((prev) => {
      if (prev[friendUuid]) return prev;
      commands.getSkinUrl(friendUuid).then((res) => {
        if (res.ok) setFriendsSkins((p) => ({ ...p, [friendUuid]: res.value }));
      });
      return prev;
    });
  }, []);

  const applyList = useCallback(
    (list: FriendsList) => {
      setFriendsList(list);
      for (const f of [
        ...(list.friends ?? []),
        ...(list.incomingRequests ?? []),
        ...(list.outgoingRequests ?? []),
      ]) {
        loadSkinFor(f.profileId);
      }
    },
    [loadSkinFor],
  );

  useEffect(() => {
    if (!uuid) return;
    let cancelled = false;
    commands.getFriends(uuid).then((res) => {
      if (cancelled) return;
      if (res.ok) {
        applyList(res.value);
        setFriendsError(null);
      } else {
        setFriendsError(formatError(res.error));
      }
    });
    return () => {
      cancelled = true;
    };
  }, [uuid, applyList]);

  useEffect(() => {
    if (!uuid) return;
    let cancelled = false;

    const tick = async () => {
      const reqId = ++presenceReqId.current;
      const res = await commands.updatePresence(
        uuid,
        currentActivity.status,
        currentActivity.joinInfo,
      );
      if (cancelled || reqId !== presenceReqId.current) return;
      let nextDelay = PRESENCE_INTERVAL_MS;
      if (res.ok) {
        const byUuid: Record<string, PresenceEntry> = {};
        for (const entry of res.value) byUuid[entry.profileId] = entry;
        setFriendsPresence(byUuid);
      } else if (res.error.kind === "rateLimited") {
        nextDelay = Math.max(PRESENCE_INTERVAL_MS, res.error.retryAfterSecs * 1000);
      }
      presenceTimer.current = setTimeout(tick, nextDelay);
    };

    tick();

    return () => {
      cancelled = true;
      if (presenceTimer.current) {
        clearTimeout(presenceTimer.current);
        presenceTimer.current = null;
      }
    };
  }, [uuid, currentActivity, presenceRefresh]);

  const refreshPresence = useCallback(() => setPresenceRefresh((c) => c + 1), []);

  useEffect(() => {
    if (!uuid) return;
    let cancelled = false;
    commands.getFriendSettings(uuid).then((res) => {
      if (cancelled || !res.ok) return;
      setFriendsSettings(res.value);
    });
    return () => {
      cancelled = true;
    };
  }, [uuid]);

  const runMutation = useCallback(
    async <T>(
      op: Promise<{ ok: true; value: T } | { ok: false; error: FriendsApiError }>,
      onSuccess: (value: T) => void,
    ) => {
      const res = await op;
      if (res.ok) {
        onSuccess(res.value);
        setFriendsError(null);
      } else {
        setFriendsError(formatError(res.error));
      }
    },
    [],
  );

  const sendFriendRequest = useCallback(
    async (name: string) => {
      if (!uuid) return;
      await runMutation(commands.sendFriendRequest(uuid, name), applyList);
    },
    [uuid, runMutation, applyList],
  );

  const acceptFriendRequest = useCallback(
    async (friendUuid: string) => {
      if (!uuid) return;
      await runMutation(commands.acceptFriendRequest(uuid, friendUuid), applyList);
    },
    [uuid, runMutation, applyList],
  );

  const removeFriend = useCallback(
    async (friendUuid: string) => {
      if (!uuid) return;
      await runMutation(commands.removeFriend(uuid, friendUuid), applyList);
    },
    [uuid, runMutation, applyList],
  );

  const updateFriendSettings = useCallback(
    async (show: boolean, accept: boolean) => {
      if (!uuid) return;
      await runMutation(commands.updateFriendSettings(uuid, show, accept), setFriendsSettings);
    },
    [uuid, runMutation],
  );

  const clearFriendsError = useCallback(() => setFriendsError(null), []);

  const friendsSorted = useMemo(() => {
    const arr = friendsList.friends ?? [];
    return [...arr].sort((a, b) => {
      const pa = friendsPresence[a.profileId];
      const pb = friendsPresence[b.profileId];
      const aOffline = isOffline(pa);
      const bOffline = isOffline(pb);
      if (aOffline !== bOffline) return aOffline ? 1 : -1;
      const ta = pa?.lastUpdated ? Date.parse(pa.lastUpdated) : 0;
      const tb = pb?.lastUpdated ? Date.parse(pb.lastUpdated) : 0;
      return tb - ta;
    });
  }, [friendsList.friends, friendsPresence]);

  return {
    friendsList,
    friendsSorted,
    friendsError,
    friendsSkins,
    friendsPresence,
    friendsSettings,
    sendFriendRequest,
    acceptFriendRequest,
    removeFriend,
    updateFriendSettings,
    refreshPresence,
    clearFriendsError,
    setCurrentActivity,
  };
};

export type { Friend, FriendSettings, PresenceEntry };
