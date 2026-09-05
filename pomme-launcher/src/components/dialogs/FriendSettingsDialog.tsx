import { Dialog, Switch } from "radix-ui";
import { useState } from "react";
import { useAppStore } from "../../lib/store";
import SettingRow from "../SettingRow";

export type FriendSettingsDialogProps = Record<string, never>;

const DEFAULT_FRIEND_SETTINGS = { show_in_list: true, accept_invites: true };

export function FriendSettingsDialog(_props: FriendSettingsDialogProps) {
  const friendsSettings = useAppStore((state) => state.friendsSettings);
  const updateFriendSettings = useAppStore((state) => state.updateFriendSettings);
  const setOpenedDialog = useAppStore((state) => state.setOpenedDialog);
  const [pending, setPending] = useState(false);

  const loading = friendsSettings === null;
  const settings = friendsSettings ?? DEFAULT_FRIEND_SETTINGS;
  const disabled = loading || pending;

  const apply = async (showInList: boolean, acceptInvites: boolean) => {
    if (disabled) {
      return;
    }
    setPending(true);
    try {
      await updateFriendSettings(showInList, acceptInvites);
    } finally {
      setPending(false);
    }
  };

  return (
    <>
      <Dialog.Title className="dialog-title">Friend Settings</Dialog.Title>

      <div className="dialog-fields">
        <SettingRow
          label="Show in Friends List"
          desc="Other players can see you in their friends lists"
        >
          <Switch.Root
            className="switch"
            checked={settings.show_in_list}
            disabled={disabled}
            onCheckedChange={(checked) => apply(checked, settings.accept_invites)}
          >
            <Switch.Thumb className="switch-thumb" />
          </Switch.Root>
        </SettingRow>
        <SettingRow label="Allow Requests" desc="Other players can send you friend requests">
          <Switch.Root
            className="switch"
            checked={settings.accept_invites}
            disabled={disabled}
            onCheckedChange={(checked) => apply(settings.show_in_list, checked)}
          >
            <Switch.Thumb className="switch-thumb" />
          </Switch.Root>
        </SettingRow>
      </div>

      <div className="dialog-actions">
        <button className="button-primary" onClick={() => setOpenedDialog(null)}>
          Close
        </button>
      </div>
    </>
  );
}
