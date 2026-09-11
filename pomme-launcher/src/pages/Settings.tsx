import { Switch } from "radix-ui";
import SettingRow from "../components/SettingRow";
import { useAppStore } from "../lib/store";

export default function SettingsPage() {
  const launcherSettings = useAppStore((state) => state.launcherSettings);
  const setKeepLauncherOpen = useAppStore((state) => state.setKeepLauncherOpen);
  const setLaunchWithConsole = useAppStore((state) => state.setLaunchWithConsole);

  return (
    <div className="page">
      <h2 className="page-heading mb-6">SETTINGS</h2>

      <section className="mb-7">
        <h3 className="label-caps mb-2.5 uppercase">General</h3>

        <SettingRow label="Language" desc="Display language for the launcher">
          <button className="button-secondary min-w-24 cursor-default text-foreground">
            {launcherSettings.language}
          </button>
        </SettingRow>

        <SettingRow label="Keep launcher open" desc="Keep the launcher open after the game starts">
          <Switch.Root
            className="switch"
            checked={launcherSettings.keepLauncherOpen}
            onCheckedChange={setKeepLauncherOpen}
          >
            <Switch.Thumb className="switch-thumb" />
          </Switch.Root>
        </SettingRow>

        <SettingRow
          label="Launch with console"
          desc="Automatically open a window with all output from the client- useful when debugging."
        >
          <Switch.Root
            className="switch"
            checked={launcherSettings.launchWithConsole}
            onCheckedChange={setLaunchWithConsole}
          >
            <Switch.Thumb className="switch-thumb" />
          </Switch.Root>
        </SettingRow>
      </section>
    </div>
  );
}
