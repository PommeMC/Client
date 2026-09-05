import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import {
  ChevronDown,
  Copy,
  Home,
  LayoutGrid,
  LogOut,
  LucideIcon,
  Newspaper,
  ChevronsLeft,
  ChevronsRight,
  Puzzle,
  Server,
  Settings,
  Trash2,
  UserPlus,
  Users,
} from "lucide-react";
import { Popover, Tooltip } from "radix-ui";
import { useEffect, useState } from "react";
import { getAccount, useAppStore } from "../lib/store";
import { Page } from "../lib/types";

const COPIED_RESET_DELAY_MS = 2000;
const ICON_SIZE = 14;
const NAV_ICON_SIZE = 18;
const TOGGLE_HINT_SIZE = 11;
const REMOVE_ICON_SIZE = 12;
const TOOLTIP_DELAY_MS = 200;
const menuButtonClass =
  "flex w-full items-center gap-2 rounded-lg px-2.5 py-2 text-xs font-medium text-muted transition-colors duration-150 hover:bg-white/[0.06] disabled:cursor-wait disabled:opacity-50";

interface NavItem {
  id: Page;
  label: string;
  icon: LucideIcon;
  soon?: boolean;
}

const NAV_ITEMS: Array<NavItem> = [
  { id: "home", label: "HOME", icon: Home },
  { id: "installations", label: "INSTALLATIONS", icon: LayoutGrid },
  { id: "servers", label: "SERVERS", icon: Server },
  { id: "friends", label: "FRIENDS", icon: Users },
  { id: "mods", label: "MODS", icon: Puzzle, soon: true },
  { id: "news", label: "NEWS & UPDATES", icon: Newspaper },
];

interface NavProps {
  startAddAccount: () => void;
  switchAccount: (index: number) => void;
  removeAccount: (uuid: string) => void;
}

export default function Navbar({ startAddAccount, switchAccount, removeAccount }: NavProps) {
  const account = useAppStore(getAccount);
  const accounts = useAppStore((state) => state.accounts);
  const activeIndex = useAppStore((state) => state.activeIndex);
  const page = useAppStore((state) => state.page);
  const setPage = useAppStore((state) => state.setPage);
  const skinUrl = useAppStore((state) => state.skinUrl);
  const authLoading = useAppStore((state) => state.authLoading);
  const authUrl = useAppStore((state) => state.authUrl);
  const accountMenuOpen = useAppStore((state) => state.accountMenuOpen);
  const setAccountMenuOpen = useAppStore((state) => state.setAccountMenuOpen);
  const collapsed = useAppStore((state) => state.sidebarCollapsed);
  const setSidebarCollapsed = useAppStore((state) => state.setSidebarCollapsed);
  const incomingCount = useAppStore((state) => state.friendsList.incomingRequests?.length ?? 0);

  const [copied, setCopied] = useState(false);
  useEffect(() => {
    if (!copied) {
      return;
    }
    const timer = setTimeout(() => setCopied(false), COPIED_RESET_DELAY_MS);
    return () => clearTimeout(timer);
  }, [copied]);

  const copyAuthUrl = () => {
    const hasAuthUrl = authUrl !== null;
    if (!hasAuthUrl) {
      return;
    }
    writeText(authUrl)
      .then(() => setCopied(true))
      .catch((error) => console.error("Failed to copy sign-in link:", error));
  };

  const openSettings = () => {
    setPage("settings");
    setAccountMenuOpen(false);
  };

  const logOut = () => {
    const isSignedIn = account !== null;
    if (!isSignedIn) {
      return;
    }
    removeAccount(account.uuid);
  };

  const toggleSidebar = () => {
    setSidebarCollapsed(!collapsed);
  };

  const hasIncomingRequests = incomingCount > 0;
  const ToggleHint = collapsed ? ChevronsRight : ChevronsLeft;

  return (
    <nav
      className={`relative flex shrink-0 flex-col overflow-hidden border-r border-white/[0.06] bg-panel/75 whitespace-nowrap backdrop-blur-xl transition-[width] duration-300 ease-out-soft ${
        collapsed ? "w-[72px]" : "w-[236px]"
      }`}
    >
      <div className="flex items-center justify-center px-3 pt-5 pb-2">
        <button
          className={`group relative flex shrink-0 flex-col items-center justify-center gap-1 rounded-xl transition-colors duration-150 hover:bg-white/[0.06] ${collapsed ? "size-12" : "px-4 py-2"}`}
          onClick={toggleSidebar}
        >
          <span className="flex items-center">
            {!collapsed && (
              <span className="text-[17px] leading-none font-bold tracking-[0.12em] text-foreground">
                P
              </span>
            )}
            <img
              src="/pomme.png"
              alt={collapsed ? "Pomme" : "O"}
              draggable={false}
              className={`transition-transform duration-200 ease-out-soft [image-rendering:pixelated] group-hover:scale-110 group-hover:-rotate-6 ${
                collapsed ? "size-8" : "mx-0.5 size-[22px]"
              }`}
            />
            {!collapsed && (
              <span className="text-[17px] leading-none font-bold tracking-[0.12em] text-foreground">
                MME
              </span>
            )}
          </span>
          {!collapsed && (
            <span className="text-[9px] leading-none font-medium tracking-[0.28em] text-muted">
              LAUNCHER
            </span>
          )}
          <span className="absolute right-0 bottom-0 flex size-[18px] items-center justify-center rounded-full border border-white/10 bg-panel text-muted opacity-0 transition-opacity duration-150 group-hover:opacity-100">
            <ToggleHint size={TOGGLE_HINT_SIZE} />
          </span>
        </button>
      </div>

      <Tooltip.Provider delayDuration={TOOLTIP_DELAY_MS}>
        <div className="relative flex flex-1 flex-col justify-center gap-1 px-3 py-2">
          {NAV_ITEMS.map((item) => {
            const isActive = page === item.id;
            const isFriends = item.id === "friends";
            const showBadge = isFriends && hasIncomingRequests;
            const showSoon = item.soon === true && !collapsed;
            const Icon = item.icon;
            return (
              <Tooltip.Root key={item.id} open={collapsed ? undefined : false}>
                <Tooltip.Trigger asChild>
                  <button
                    data-active={isActive}
                    className={`relative flex h-11 items-center gap-3.5 rounded-xl text-left text-[12px] font-semibold tracking-[0.08em] transition-colors duration-150 ${
                      collapsed ? "justify-center px-0" : "px-4"
                    } ${
                      isActive
                        ? "bg-white/[0.06] text-foreground"
                        : "text-muted hover:bg-white/[0.04] hover:text-foreground"
                    }`}
                    onClick={() => setPage(item.id)}
                  >
                    <Icon size={NAV_ICON_SIZE} className="shrink-0" />
                    {!collapsed && <span className="truncate">{item.label}</span>}
                    {showSoon && (
                      <span className="ml-auto rounded-full bg-white/[0.06] px-1.5 py-0.5 text-[8px] leading-none tracking-[0.1em] text-faint">
                        SOON
                      </span>
                    )}
                    {showBadge && !collapsed && (
                      <span className="ml-auto min-w-4 rounded-full bg-green px-1.5 py-0.5 text-center text-[9px] leading-none font-semibold text-on-green">
                        {incomingCount}
                      </span>
                    )}
                    {showBadge && collapsed && (
                      <span className="absolute top-1.5 right-2 size-1.5 rounded-full bg-green" />
                    )}
                  </button>
                </Tooltip.Trigger>
                <Tooltip.Portal>
                  <Tooltip.Content side="right" sideOffset={12} className="tooltip">
                    {item.label}
                  </Tooltip.Content>
                </Tooltip.Portal>
              </Tooltip.Root>
            );
          })}
        </div>
      </Tooltip.Provider>

      <div className="flex flex-col gap-2 p-3">
        {account ? (
          <Popover.Root open={accountMenuOpen} onOpenChange={setAccountMenuOpen}>
            <Popover.Trigger asChild>
              <button
                className={`flex w-full items-center gap-2.5 rounded-xl border border-white/[0.08] bg-black/25 py-2 shadow-[inset_0_1px_0_rgba(255,255,255,0.06)] backdrop-blur-xl transition-all duration-200 ease-out-soft hover:border-white/15 hover:bg-black/15 data-[state=open]:border-white/15 data-[state=open]:bg-black/15 ${
                  collapsed ? "justify-center px-0" : "px-2.5"
                }`}
              >
                <div
                  className="skin-head"
                  style={skinUrl ? { backgroundImage: `url("${skinUrl}")` } : undefined}
                />
                {!collapsed && (
                  <span className="min-w-0 flex-1 truncate text-left text-[13px] font-medium text-foreground">
                    {account.username}
                  </span>
                )}
                {!collapsed && (
                  <ChevronDown
                    size={ICON_SIZE}
                    className={`shrink-0 text-faint transition-transform duration-200 ${
                      accountMenuOpen ? "rotate-180" : ""
                    }`}
                  />
                )}
              </button>
            </Popover.Trigger>
            <Popover.Portal>
              <Popover.Content
                side="top"
                align="start"
                sideOffset={8}
                className={`menu origin-(--radix-popover-content-transform-origin) outline-none ${
                  collapsed ? "w-[208px]" : "w-(--radix-popover-trigger-width)"
                }`}
              >
                {accounts.map((existing, index) => {
                  const isActive = index === activeIndex;
                  return (
                    <div key={existing.uuid} className="flex items-center gap-1">
                      <button
                        className={`min-w-0 flex-1 truncate rounded-lg px-2.5 py-2 text-left text-xs font-medium transition-colors duration-150 hover:bg-white/[0.06] ${
                          isActive ? "text-green" : "text-foreground"
                        }`}
                        onClick={() => switchAccount(index)}
                      >
                        {existing.username}
                      </button>
                      <button
                        className="rounded-md p-2 text-faint transition-colors duration-150 hover:bg-white/[0.06] hover:text-red"
                        onClick={() => removeAccount(existing.uuid)}
                      >
                        <Trash2 size={REMOVE_ICON_SIZE} />
                      </button>
                    </div>
                  );
                })}
                <div className="my-1 h-px bg-line" />
                <button
                  className={`${menuButtonClass} hover:text-foreground`}
                  onClick={startAddAccount}
                  disabled={authLoading}
                >
                  <UserPlus size={ICON_SIZE} />
                  <span>{authLoading ? "Signing in..." : "Add account"}</span>
                </button>
                <button
                  className={`${menuButtonClass} hover:text-foreground`}
                  onClick={openSettings}
                >
                  <Settings size={ICON_SIZE} />
                  <span>Settings</span>
                </button>
                <button className={`${menuButtonClass} hover:text-red`} onClick={logOut}>
                  <LogOut size={ICON_SIZE} />
                  <span>Log out</span>
                </button>
              </Popover.Content>
            </Popover.Portal>
          </Popover.Root>
        ) : (
          <button
            className={`button-primary h-10 w-full text-[11px] font-bold tracking-[0.14em] ${
              collapsed ? "px-0" : ""
            }`}
            onClick={startAddAccount}
            disabled={authLoading}
          >
            {authLoading ? "Signing in..." : "SIGN IN"}
          </button>
        )}
        {authLoading && authUrl && !collapsed && (
          <button className="button-secondary w-full text-[11px]" onClick={copyAuthUrl}>
            <Copy size={ICON_SIZE} />
            <span>{copied ? "Copied!" : "Link didn't open? Copy it"}</span>
          </button>
        )}
      </div>
    </nav>
  );
}
