import { useState } from "react";
import { useAppStateContext } from "../../lib/state";

export type FriendSettingsDialogProps = Record<string, never>;

export function FriendSettingsDialog(_props: FriendSettingsDialogProps) {
  const { friendsSettings, updateFriendSettings, setOpenedDialog } = useAppStateContext();
  const [pending, setPending] = useState(false);

  const loading = friendsSettings === null;
  const settings = friendsSettings ?? { show_in_list: true, accept_invites: true };

  const apply = async (show: boolean, accept: boolean) => {
    if (loading || pending) return;
    setPending(true);
    try {
      await updateFriendSettings(show, accept);
    } finally {
      setPending(false);
    }
  };

  return (
    <div className="dialog" onClick={(e) => e.stopPropagation()}>
      <h2 className="dialog-title">Friend Settings</h2>

      <div className="dialog-fields">
        <SettingRow
          label="Show in Friends List"
          desc="Other players can see you in their friends lists"
          value={settings.show_in_list}
          disabled={loading || pending}
          onToggle={() => apply(!settings.show_in_list, settings.accept_invites)}
        />
        <SettingRow
          label="Allow Requests"
          desc="Other players can send you friend requests"
          value={settings.accept_invites}
          disabled={loading || pending}
          onToggle={() => apply(settings.show_in_list, !settings.accept_invites)}
        />
      </div>

      <div className="dialog-actions">
        <button className="dialog-confirm" onClick={() => setOpenedDialog(null)}>
          Close
        </button>
      </div>
    </div>
  );
}

function SettingRow({
  label,
  desc,
  value,
  disabled,
  onToggle,
}: {
  label: string;
  desc: string;
  value: boolean;
  disabled: boolean;
  onToggle: () => void;
}) {
  return (
    <div className="settings-row">
      <div className="settings-row-info">
        <span className="settings-row-label">{label}</span>
        <span className="settings-row-desc">{desc}</span>
      </div>
      <div className="settings-row-control">
        <button
          className={`settings-toggle ${value ? "on" : ""}`}
          disabled={disabled}
          onClick={onToggle}
        >
          <div className="settings-toggle-knob" />
        </button>
      </div>
    </div>
  );
}
