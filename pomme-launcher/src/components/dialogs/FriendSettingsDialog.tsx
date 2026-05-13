import { useState } from "react";
import { useAppStateContext } from "../../lib/state";

export type FriendSettingsDialogProps = Record<string, never>;

export function FriendSettingsDialog(_props: FriendSettingsDialogProps) {
  const { friendsSettings, updateFriendSettings, setOpenedDialog } = useAppStateContext();
  const [pending, setPending] = useState(false);

  const loading = friendsSettings === null;
  const settings = friendsSettings ?? { show_in_list: true, accept_invites: true };

  const toggle = async (next: { show_in_list?: boolean; accept_invites?: boolean }) => {
    if (loading || pending) return;
    setPending(true);
    try {
      await updateFriendSettings(
        next.show_in_list ?? settings.show_in_list,
        next.accept_invites ?? settings.accept_invites,
      );
    } finally {
      setPending(false);
    }
  };

  return (
    <div className="dialog" onClick={(e) => e.stopPropagation()}>
      <h2 className="dialog-title">Friend Settings</h2>

      <div className="dialog-fields">
        <div className="settings-row">
          <div className="settings-row-info">
            <span className="settings-row-label">Show in Friends List</span>
            <span className="settings-row-desc">
              Other players can see you in their friends lists
            </span>
          </div>
          <div className="settings-row-control">
            <button
              className={`settings-toggle ${settings.show_in_list ? "on" : ""}`}
              disabled={loading || pending}
              onClick={() => toggle({ show_in_list: !settings.show_in_list })}
            >
              <div className="settings-toggle-knob" />
            </button>
          </div>
        </div>

        <div className="settings-row">
          <div className="settings-row-info">
            <span className="settings-row-label">Allow Requests</span>
            <span className="settings-row-desc">Other players can send you friend requests</span>
          </div>
          <div className="settings-row-control">
            <button
              className={`settings-toggle ${settings.accept_invites ? "on" : ""}`}
              disabled={loading || pending}
              onClick={() => toggle({ accept_invites: !settings.accept_invites })}
            >
              <div className="settings-toggle-knob" />
            </button>
          </div>
        </div>
      </div>

      <div className="dialog-actions">
        <button className="dialog-confirm" onClick={() => setOpenedDialog(null)}>
          Close
        </button>
      </div>
    </div>
  );
}
