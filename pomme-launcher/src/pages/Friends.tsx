import { HiCheck, HiPlus, HiXMark } from "react-icons/hi2";
import { Friend } from "../lib/friends";
import { useAppStateContext } from "../lib/state";

export default function FriendsPage() {
  const {
    account,
    friendsList,
    friendsError,
    friendsSkins,
    sendFriendRequest,
    acceptFriendRequest,
    removeFriend,
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

  const friends = friendsList.friends ?? [];
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
        <button className="servers-add-btn" onClick={openAddDialog}>
          <HiPlus /> Add Friend
        </button>
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
        emptyMessage="You haven't added any friends yet."
        renderActions={(uuid) => (
          <button className="friends-btn" onClick={() => removeFriend(uuid)} title="Remove friend">
            <HiXMark /> Remove
          </button>
        )}
      />

      <FriendsSection
        title="Incoming Requests"
        friends={incoming}
        skinUrls={friendsSkins}
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
  emptyMessage,
  hideWhenEmpty,
  renderActions,
}: {
  title: string;
  friends: Friend[];
  skinUrls: Record<string, string>;
  emptyMessage?: string;
  hideWhenEmpty?: boolean;
  renderActions: (uuid: string) => React.ReactNode;
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
          <FriendRow key={f.profileId} friend={f} skinUrl={skinUrls[f.profileId]}>
            {renderActions(f.profileId)}
          </FriendRow>
        ))}
      </div>
    </>
  );
}

function FriendRow({
  friend,
  skinUrl,
  children,
}: {
  friend: Friend;
  skinUrl: string | undefined;
  children: React.ReactNode;
}) {
  return (
    <div className="mock-friend">
      <div
        className="mc-head"
        style={skinUrl ? { backgroundImage: `url(${skinUrl})` } : undefined}
      />
      <div className="mock-friend-info">
        <span className="mock-friend-name">{friend.name}</span>
      </div>
      <div className="friends-actions">{children}</div>
    </div>
  );
}
