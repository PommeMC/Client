import { useCallback, useEffect, useState } from "react";
import { commands } from "../bindings";
import { Friend, FriendsList } from "../bindings/pomme_launcher/friends";

const EMPTY: FriendsList = { friends: [], incomingRequests: [], outgoingRequests: [] };

export const useFriends = (uuid: string | null) => {
  const [friendsList, setFriendsList] = useState<FriendsList>(EMPTY);
  const [friendsLoaded, setFriendsLoaded] = useState(false);
  const [friendsError, setFriendsError] = useState<string | null>(null);
  const [friendsSkins, setFriendsSkins] = useState<Record<string, string>>({});

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

  const runMutation = useCallback(
    async (op: Promise<{ ok: true; value: FriendsList } | { ok: false; error: string }>) => {
      const res = await op;
      if (res.ok) {
        applyList(res.value);
        setFriendsError(null);
      } else {
        setFriendsError(res.error);
      }
    },
    [applyList],
  );

  const sendFriendRequest = useCallback(
    async (name: string) => {
      if (!uuid) return;
      await runMutation(commands.sendFriendRequest(uuid, name));
    },
    [uuid, runMutation],
  );

  const acceptFriendRequest = useCallback(
    async (friendUuid: string) => {
      if (!uuid) return;
      await runMutation(commands.acceptFriendRequest(uuid, friendUuid));
    },
    [uuid, runMutation],
  );

  const removeFriend = useCallback(
    async (friendUuid: string) => {
      if (!uuid) return;
      await runMutation(commands.removeFriend(uuid, friendUuid));
    },
    [uuid, runMutation],
  );

  const clearFriendsError = useCallback(() => setFriendsError(null), []);

  return {
    friendsList,
    friendsLoaded,
    friendsError,
    friendsSkins,
    sendFriendRequest,
    acceptFriendRequest,
    removeFriend,
    clearFriendsError,
  };
};

export type { Friend };
