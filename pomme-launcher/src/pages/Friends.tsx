import { Check, Play, Plus, RefreshCw, Settings, X } from "lucide-react";
import { ReactNode } from "react";
import { formatStatus, Friend, isOffline, PresenceEntry } from "../lib/friends";
import { getAccount, useAppStore } from "../lib/store";
import { handleLaunchType } from "../lib/types";

const ICON_SIZE = 14;
const BUTTON_ICON_SIZE = 12;
const JOIN_ADDRESS_PATTERN = /^[a-zA-Z0-9.\-:_[\]]+$/;
const friendButtonClass = "button-secondary h-[26px] gap-1 px-2.5 text-[11px] font-semibold";
const acceptButtonClass = `${friendButtonClass} border-green/40 text-green hover:enabled:border-green hover:enabled:text-green`;

function getJoinAddress(presence: PresenceEntry | undefined): string | null {
  const hasPresence = presence !== undefined;
  if (!hasPresence) {
    return null;
  }
  const isOnServer = presence.status === "PLAYING_SERVER";
  if (!isOnServer) {
    return null;
  }
  const rawAddress = presence.joinInfo?.value ?? "";
  const isSafeAddress = JOIN_ADDRESS_PATTERN.test(rawAddress);
  if (!isSafeAddress) {
    return null;
  }
  return rawAddress;
}

interface FriendRowProps {
  friend: Friend;
  skinUrl: string | undefined;
  presence: PresenceEntry | undefined;
  children: ReactNode;
}

function FriendRow({ friend, skinUrl, presence, children }: FriendRowProps) {
  const offline = isOffline(presence);
  return (
    <div className="row gap-3 px-3.5 py-3">
      <div
        className={`skin-head ${offline ? "opacity-60 grayscale-[0.8]" : ""}`}
        style={skinUrl ? { backgroundImage: `url("${skinUrl}")` } : undefined}
      />
      <div className="flex min-w-0 flex-1 flex-col gap-0.5">
        <span className={`text-[13px] font-medium ${offline ? "text-muted" : "text-foreground"}`}>
          {friend.name}
        </span>
        <span className="text-xs text-muted">{formatStatus(presence)}</span>
      </div>
      <div className={`size-1.5 shrink-0 ${offline ? "bg-line-strong" : "bg-green"}`} />
      <div className="flex shrink-0 items-center gap-1.5">{children}</div>
    </div>
  );
}

interface FriendsSectionProps {
  title: string;
  friends: Friend[];
  skinUrls: Record<string, string>;
  presence: Record<string, PresenceEntry>;
  emptyMessage?: string;
  hideWhenEmpty?: boolean;
  renderActions: (uuid: string, presence: PresenceEntry | undefined) => ReactNode;
}

function FriendsSection({
  title,
  friends,
  skinUrls,
  presence,
  emptyMessage,
  hideWhenEmpty,
  renderActions,
}: FriendsSectionProps) {
  const isEmpty = friends.length === 0;
  const hidden = hideWhenEmpty && isEmpty;
  if (hidden) {
    return null;
  }
  const showEmptyMessage = isEmpty && emptyMessage !== undefined;

  return (
    <>
      <h3 className="label-caps mt-6 mb-2.5 uppercase first-of-type:mt-0">
        {title} — {friends.length}
      </h3>
      <div className="list">
        {showEmptyMessage && <p className="empty-text">{emptyMessage}</p>}
        {friends.map((friend) => (
          <FriendRow
            key={friend.profileId}
            friend={friend}
            skinUrl={skinUrls[friend.profileId]}
            presence={presence[friend.profileId]}
          >
            {renderActions(friend.profileId, presence[friend.profileId])}
          </FriendRow>
        ))}
      </div>
    </>
  );
}

export default function FriendsPage({ handleLaunch }: { handleLaunch: handleLaunchType }) {
  const account = useAppStore(getAccount);
  const friendsList = useAppStore((state) => state.friendsList);
  const friendsSorted = useAppStore((state) => state.friendsSorted);
  const friendsError = useAppStore((state) => state.friendsError);
  const friendsSkins = useAppStore((state) => state.friendsSkins);
  const friendsPresence = useAppStore((state) => state.friendsPresence);
  const sendFriendRequest = useAppStore((state) => state.sendFriendRequest);
  const acceptFriendRequest = useAppStore((state) => state.acceptFriendRequest);
  const removeFriend = useAppStore((state) => state.removeFriend);
  const refreshPresence = useAppStore((state) => state.refreshPresence);
  const clearFriendsError = useAppStore((state) => state.clearFriendsError);
  const setOpenedDialog = useAppStore((state) => state.setOpenedDialog);

  const isSignedIn = account !== null;
  if (!isSignedIn) {
    return (
      <div className="page">
        <h2 className="page-heading mb-6">FRIENDS</h2>
        <p className="empty-text">Sign in to view your friends list.</p>
      </div>
    );
  }

  const incoming = friendsList.incomingRequests ?? [];
  const outgoing = friendsList.outgoingRequests ?? [];

  const openAddDialog = () => {
    setOpenedDialog({ name: "add_friend_dialog", props: { onSubmit: sendFriendRequest } });
  };

  const openSettingsDialog = () => {
    setOpenedDialog({ name: "friend_settings_dialog", props: {} });
  };

  return (
    <div className="page">
      <div className="page-header">
        <h2 className="page-heading">FRIENDS</h2>
        <div className="flex items-center gap-2">
          <button
            className="button-secondary button-icon"
            onClick={refreshPresence}
            title="Refresh presence"
          >
            <RefreshCw size={ICON_SIZE} />
          </button>
          <button
            className="button-secondary button-icon"
            onClick={openSettingsDialog}
            title="Friend settings"
          >
            <Settings size={ICON_SIZE} />
          </button>
          <button className="button-primary" onClick={openAddDialog}>
            <Plus size={ICON_SIZE} /> Add Friend
          </button>
        </div>
      </div>

      {friendsError && (
        <div
          className="mb-4 cursor-pointer border border-red/50 px-3.5 py-2.5 text-xs font-medium text-red"
          onClick={clearFriendsError}
        >
          {friendsError}
        </div>
      )}

      <FriendsSection
        title="Friends"
        friends={friendsSorted}
        skinUrls={friendsSkins}
        presence={friendsPresence}
        emptyMessage="You haven't added any friends yet."
        renderActions={(uuid, presence) => {
          const joinAddress = getJoinAddress(presence);
          const canJoin = joinAddress !== null;
          return (
            <>
              {canJoin && (
                <button
                  className={acceptButtonClass}
                  onClick={() => handleLaunch({ serverIp: joinAddress })}
                  title={`Join ${joinAddress}`}
                >
                  <Play size={BUTTON_ICON_SIZE} fill="currentColor" /> Join
                </button>
              )}
              <button
                className={friendButtonClass}
                onClick={() => removeFriend(uuid)}
                title="Remove friend"
              >
                <X size={BUTTON_ICON_SIZE} /> Remove
              </button>
            </>
          );
        }}
      />

      <FriendsSection
        title="Incoming Requests"
        friends={incoming}
        skinUrls={friendsSkins}
        presence={friendsPresence}
        hideWhenEmpty
        renderActions={(uuid) => (
          <>
            <button
              className={acceptButtonClass}
              onClick={() => acceptFriendRequest(uuid)}
              title="Accept"
            >
              <Check size={BUTTON_ICON_SIZE} /> Accept
            </button>
            <button
              className={friendButtonClass}
              onClick={() => removeFriend(uuid)}
              title="Decline"
            >
              <X size={BUTTON_ICON_SIZE} /> Decline
            </button>
          </>
        )}
      />

      <FriendsSection
        title="Outgoing Requests"
        friends={outgoing}
        skinUrls={friendsSkins}
        presence={friendsPresence}
        hideWhenEmpty
        renderActions={(uuid) => (
          <button
            className={friendButtonClass}
            onClick={() => removeFriend(uuid)}
            title="Cancel request"
          >
            <X size={BUTTON_ICON_SIZE} /> Cancel
          </button>
        )}
      />
    </div>
  );
}
