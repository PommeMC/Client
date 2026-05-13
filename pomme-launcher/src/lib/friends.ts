import { useCallback, useEffect, useState } from "react";
import { commands } from "../bindings";
import {
  Friend,
  FriendSettings,
  FriendsList,
  PresenceEntry,
} from "../bindings/pomme_launcher/friends";

const EMPTY: FriendsList = { friends: [], incomingRequests: [], outgoingRequests: [] };
const PRESENCE_INTERVAL_MS = 30_000;

export const useFriends = (uuid: string | null) => {
  const [friendsList, setFriendsList] = useState<FriendsList>(EMPTY);
  const [friendsLoaded, setFriendsLoaded] = useState(false);
  const [friendsError, setFriendsError] = useState<string | null>(null);
  const [friendsSkins, setFriendsSkins] = useState<Record<string, string>>({});
  const [friendsPresence, setFriendsPresence] = useState<Record<string, PresenceEntry>>({});
  const [friendsSettings, setFriendsSettings] = useState<FriendSettings | null>(null);

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
        setFriendsError(res.error);
      }
      setFriendsLoaded(true);
    });
    return () => {
      cancelled = true;
    };
  }, [uuid, applyList]);

  const refreshPresence = useCallback(() => {
    if (!uuid) return;
    commands.updatePresence(uuid).then((res) => {
      if (!res.ok) return;
      const byUuid: Record<string, PresenceEntry> = {};
      for (const entry of res.value) {
        byUuid[entry.profileId] = entry;
      }
      setFriendsPresence(byUuid);
    });
  }, [uuid]);

  useEffect(() => {
    if (!uuid) return;
    refreshPresence();
    const interval = setInterval(refreshPresence, PRESENCE_INTERVAL_MS);
    return () => clearInterval(interval);
  }, [uuid, refreshPresence]);

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
      op: Promise<{ ok: true; value: T } | { ok: false; error: string }>,
      onSuccess: (value: T) => void,
    ) => {
      const res = await op;
      if (res.ok) {
        onSuccess(res.value);
        setFriendsError(null);
      } else {
        setFriendsError(res.error);
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

  return {
    friendsList,
    friendsLoaded,
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
  };
};

export type { Friend, FriendSettings, PresenceEntry };
