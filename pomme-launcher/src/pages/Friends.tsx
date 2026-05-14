import { HiArrowPath, HiCheck, HiCog6Tooth, HiPlay, HiPlus, HiXMark } from "react-icons/hi2";
import { Friend, isOffline, PresenceEntry } from "../lib/friends";
import { useAppStateContext } from "../lib/state";
import { handleLaunchType } from "../lib/types";

export default function FriendsPage({ handleLaunch }: { handleLaunch: handleLaunchType }) {
  const {
    account,
    friendsList,
    friendsSorted,
    friendsError,
    friendsSkins,
    friendsPresence,
    sendFriendRequest,
    acceptFriendRequest,
    removeFriend,
    refreshPresence,
    clearFriendsError,
    setOpenedDialog,
  } = useAppStateContext();

  if (!account) {
    return (
      <div className="page friends-page">
        <h2 className="page-heading">FRIENDS</h2>
        <p className="servers-empty">Sign in to view your friends list.</p>
      </div>
    );
  }

  const friends = friendsSorted;
  const incoming = friendsList.incomingRequests ?? [];
  const outgoing = friendsList.outgoingRequests ?? [];

  const openAddDialog = () =>
    setOpenedDialog({
      name: "add_friend_dialog",
      props: { onSubmit: sendFriendRequest },
    });

  return (
    <div className="page friends-page">
      <div className="friends-header">
        <h2 className="page-heading">FRIENDS</h2>
        <div className="friends-header-actions">
          <button
            className="friends-settings-btn"
            onClick={refreshPresence}
            title="Refresh presence"
          >
            <HiArrowPath />
          </button>
          <button
            className="friends-settings-btn"
            onClick={() => setOpenedDialog({ name: "friend_settings_dialog", props: {} })}
            title="Friend settings"
          >
            <HiCog6Tooth />
          </button>
          <button className="servers-add-btn" onClick={openAddDialog}>
            <HiPlus /> Add Friend
          </button>
        </div>
      </div>

      {friendsError && (
        <div className="friends-error" onClick={clearFriendsError}>
          {friendsError}
        </div>
      )}

      <FriendsSection
        title="Friends"
        friends={friends}
        skinUrls={friendsSkins}
        presence={friendsPresence}
        emptyMessage="You haven't added any friends yet."
        renderActions={(uuid, p) => {
          const rawAddr = p?.status === "PLAYING_SERVER" ? p.joinInfo?.value : undefined;
          const joinAddress =
            rawAddr && /^[a-zA-Z0-9.\-:_[\]]+$/.test(rawAddr) ? rawAddr : undefined;
          return (
            <>
              {joinAddress && (
                <button
                  className="friends-btn accept"
                  onClick={() => handleLaunch({ serverIp: joinAddress })}
                  title={`Join ${joinAddress}`}
                >
                  <HiPlay /> Join
                </button>
              )}
              <button
                className="friends-btn"
                onClick={() => removeFriend(uuid)}
                title="Remove friend"
              >
                <HiXMark /> Remove
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
              className="friends-btn accept"
              onClick={() => acceptFriendRequest(uuid)}
              title="Accept"
            >
              <HiCheck /> Accept
            </button>
            <button className="friends-btn" onClick={() => removeFriend(uuid)} title="Decline">
              <HiXMark /> Decline
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
          <button className="friends-btn" onClick={() => removeFriend(uuid)} title="Cancel request">
            <HiXMark /> Cancel
          </button>
        )}
      />
    </div>
  );
}

function FriendsSection({
  title,
  friends,
  skinUrls,
  presence,
  emptyMessage,
  hideWhenEmpty,
  renderActions,
}: {
  title: string;
  friends: Friend[];
  skinUrls: Record<string, string>;
  presence: Record<string, PresenceEntry>;
  emptyMessage?: string;
  hideWhenEmpty?: boolean;
  renderActions: (uuid: string, presence: PresenceEntry | undefined) => React.ReactNode;
}) {
  if (hideWhenEmpty && friends.length === 0) return null;

  return (
    <>
      <h3 className="mock-subheading">
        {title} — {friends.length}
      </h3>
      <div className="mock-list">
        {friends.length === 0 && emptyMessage && <p className="servers-empty">{emptyMessage}</p>}
        {friends.map((f) => (
          <FriendRow
            key={f.profileId}
            friend={f}
            skinUrl={skinUrls[f.profileId]}
            presence={presence[f.profileId]}
          >
            {renderActions(f.profileId, presence[f.profileId])}
          </FriendRow>
        ))}
      </div>
    </>
  );
}

function FriendRow({
  friend,
  skinUrl,
  presence,
  children,
}: {
  friend: Friend;
  skinUrl: string | undefined;
  presence: PresenceEntry | undefined;
  children: React.ReactNode;
}) {
  const offline = isOffline(presence);
  return (
    <div className="mock-friend">
      <div
        className={`mc-head ${offline ? "off" : ""}`}
        style={skinUrl ? { backgroundImage: `url("${skinUrl}")` } : undefined}
      />
      <div className="mock-friend-info">
        <span className={`mock-friend-name ${offline ? "off" : ""}`}>{friend.name}</span>
        <span className="mock-friend-status">{formatStatus(presence)}</span>
      </div>
      <div className={`mock-dot ${offline ? "off" : "on"}`} />
      <div className="friends-actions">{children}</div>
    </div>
  );
}

function formatStatus(presence: PresenceEntry | undefined): string {
  if (!presence || presence.status === "OFFLINE") {
    const seen = formatLastSeen(presence?.lastUpdated);
    return seen ? `Offline · ${seen}` : "Offline";
  }
  switch (presence.status) {
    case "ONLINE":
      return "Online";
    case "PLAYING_OFFLINE":
      return "In singleplayer";
    case "PLAYING_REALMS":
      return "Playing Realms";
    case "PLAYING_SERVER":
      return presence.joinInfo?.value
        ? `Playing: ${presence.joinInfo.value}`
        : "Playing multiplayer";
    case "PLAYING_HOSTED_SERVER":
      return "Hosting local world";
    default:
      return presence.status;
  }
}

function formatLastSeen(iso: string | null | undefined): string {
  if (!iso) return "";
  const then = Date.parse(iso);
  if (Number.isNaN(then)) return "";
  const deltaSec = Math.max(0, (Date.now() - then) / 1000);
  if (deltaSec < 60) return "just now";
  if (deltaSec < 3600) return `${Math.floor(deltaSec / 60)}m ago`;
  if (deltaSec < 86400) return `${Math.floor(deltaSec / 3600)}h ago`;
  if (deltaSec < 604800) return `${Math.floor(deltaSec / 86400)}d ago`;
  return new Date(then).toLocaleDateString(undefined, { month: "short", day: "numeric" });
}
