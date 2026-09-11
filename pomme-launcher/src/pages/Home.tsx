import { Box, ChevronDown, Download, Play } from "lucide-react";
import { DropdownMenu } from "radix-ui";
import { PointerEvent, useState } from "react";
import { PatchNote } from "../bindings/pomme_launcher/commands";
import SkinPreview, { PointerPosition } from "../components/SkinPreview";
import SkinRunner from "../components/SkinRunner";
import { formatStatus, isOffline } from "../lib/friends";
import { getPingText, getPlayersText } from "../lib/servers";
import { getAccount, useAppStore } from "../lib/store";
import { DownloadProgress, handleLaunchType, LaunchingStatus, Server } from "../lib/types";

const FEATURED_INDEX = 0;
const GRID_NEWS_START = 1;
const GRID_NEWS_END = 5;
const WIDE_ONLY_CARD_INDEX = 3;
const NEWS_STAGGER_MS = 70;
const NO_PROGRESS = 0;
const PLAY_ICON_SIZE = 18;
const ICON_SIZE = 14;
const CHEVRON_SIZE = 12;
const JOIN_ICON_SIZE = 12;
const PARALLAX_RANGE_PX = 14;
const POINTER_CENTER = 0.5;
const BUSY_LABELS: Record<Exclude<LaunchingStatus, null>, string> = {
  checking_assets: "Checking assets...",
  installing: "Installing...",
  launching: "Launching...",
};
const PLAY_CURSOR_LAUNCHING = "cursor-wait";
const PLAY_CURSOR_INSTALLING = "cursor-progress";
const PLAY_CURSOR_IDLE = "";
const playButtonClass =
  "button-primary h-[52px] w-full animate-breathe gap-3 rounded-xl px-8 hover:enabled:-translate-y-px hover:enabled:shadow-[inset_0_1px_0_rgba(255,255,255,0.3),0_0_0_1px_rgba(65,222,23,0.45),0_18px_40px_-12px_rgba(65,222,23,0.75)] disabled:opacity-85";
const serverRowClass =
  "group flex items-center gap-3 border-r border-b border-white/[0.06] py-3 pr-3 pl-4 text-left transition-colors duration-200 last:border-r-0 hover:bg-white/[0.04]";
const chipClass =
  "group flex items-center gap-3 rounded-xl border border-white/[0.08] bg-black/25 shadow-[inset_0_1px_0_rgba(255,255,255,0.06)] backdrop-blur-xl py-2.5 pr-2.5 pl-3.5 text-left transition-all duration-200 ease-out-soft hover:-translate-y-0.5 hover:border-white/15 hover:bg-black/15";
const newsCardClass =
  "group animate-rise cursor-pointer overflow-hidden rounded-2xl glass transition-all duration-300 ease-out-soft hover:-translate-y-0.5 hover:border-white/15 hover:bg-white/[0.06] hover:shadow-[0_18px_40px_-20px_rgba(0,0,0,0.8)]";

type PointerOffset = { x: number; y: number };

const POINTER_CENTERED: PointerOffset = { x: 0, y: 0 };

interface HomepageProps {
  handleLaunch: handleLaunchType;
  openPatchNote: (item: PatchNote) => Promise<void>;
}

function getPlayLabel(launchingStatus: LaunchingStatus, isDownloaded: boolean): string {
  const isBusy = launchingStatus !== null;
  if (isBusy) {
    const busyLabel = BUSY_LABELS[launchingStatus];
    return busyLabel;
  }
  const idleLabel = isDownloaded ? "PLAY" : "INSTALL";
  return idleLabel;
}

function getProgressFraction(downloadProgress: DownloadProgress): number {
  const hasTotal = downloadProgress.total > 0;
  if (!hasTotal) {
    return NO_PROGRESS;
  }
  const fraction = downloadProgress.downloaded / downloadProgress.total;
  return fraction;
}

function getPlayCursorClass(launchingStatus: LaunchingStatus): string {
  const isLaunching = launchingStatus === "launching";
  if (isLaunching) {
    return PLAY_CURSOR_LAUNCHING;
  }
  const isIdle = launchingStatus === null;
  if (isIdle) {
    return PLAY_CURSOR_IDLE;
  }
  return PLAY_CURSOR_INSTALLING;
}

function formatDate(date: string): string {
  const formatted = date.replace(/-/g, ".");
  return formatted;
}

function getPointerOffset(event: PointerEvent<HTMLElement>): PointerOffset {
  const bounds = event.currentTarget.getBoundingClientRect();
  const x = ((event.clientX - bounds.left) / bounds.width - POINTER_CENTER) * 2;
  const y = ((event.clientY - bounds.top) / bounds.height - POINTER_CENTER) * 2;
  const offset: PointerOffset = { x, y };
  return offset;
}

function getParallaxStyle(pointer: PointerOffset): { translate: string } {
  const x = -pointer.x * PARALLAX_RANGE_PX;
  const y = -pointer.y * PARALLAX_RANGE_PX;
  const style = { translate: `${x}px ${y}px` };
  return style;
}

export default function Homepage({ handleLaunch, openPatchNote }: HomepageProps) {
  const account = useAppStore(getAccount);
  const launchingStatus = useAppStore((state) => state.launchingStatus);
  const installations = useAppStore((state) => state.installations);
  const activeInstall = useAppStore((state) => state.activeInstall);
  const setActiveInstall = useAppStore((state) => state.setActiveInstall);
  const news = useAppStore((state) => state.news);
  const servers = useAppStore((state) => state.servers);
  const friends = useAppStore((state) => state.friendsSorted);
  const friendsSkins = useAppStore((state) => state.friendsSkins);
  const friendsPresence = useAppStore((state) => state.friendsPresence);
  const status = useAppStore((state) => state.status);
  const downloadedVersions = useAppStore((state) => state.downloadedVersions);
  const downloadProgress = useAppStore((state) => state.downloadProgress);
  const skinUrl = useAppStore((state) => state.skinUrl);
  const setOpenedDialog = useAppStore((state) => state.setOpenedDialog);

  const [pagePointer, setPagePointer] = useState<PointerPosition | null>(null);
  const [featuredPointer, setFeaturedPointer] = useState<PointerOffset>(POINTER_CENTERED);

  const isBusy = launchingStatus !== null;
  const isLaunching = launchingStatus === "launching";
  const isDownloaded = downloadedVersions.has(activeInstall?.version ?? "");
  const showPlayIcon = !isBusy && isDownloaded;
  const playLabel = getPlayLabel(launchingStatus, isDownloaded);
  const hasInstallations = installations.length > 0;
  const hasServers = servers.length > 0;
  const isSignedIn = account !== null;
  const hasFriends = isSignedIn && friends.length > 0;
  const featured = news[FEATURED_INDEX] ?? null;
  const gridNews = news.slice(GRID_NEWS_START, GRID_NEWS_END);

  const selectInstallation = (id: string) => {
    const installation = installations.find((candidate) => candidate.id === id) ?? null;
    setActiveInstall(installation);
  };

  const openNewInstallationDialog = () => {
    setOpenedDialog({ name: "installation_dialog", props: { type: "new" } });
  };

  const openFeatured = () => {
    const hasFeatured = featured !== null;
    if (!hasFeatured) {
      return;
    }
    openPatchNote(featured);
  };

  const joinServer = (server: Server) => {
    handleLaunch({ serverIp: server.ip, serverVersion: server.version });
  };

  return (
    <div
      className="page flex flex-col gap-8"
      onPointerMove={(event) => setPagePointer({ x: event.clientX, y: event.clientY })}
      onPointerLeave={() => setPagePointer(null)}
    >
      <section className="grid grid-cols-1 gap-5 @min-[900px]:grid-cols-[minmax(0,1.15fr)_minmax(0,1fr)]">
        <div className="glass relative flex min-h-[340px] overflow-hidden rounded-2xl">
          <div className="pointer-events-none absolute inset-0 bg-[radial-gradient(45%_55%_at_18%_100%,rgba(65,222,23,0.16),transparent_70%)]" />
          <div className="relative flex min-w-0 flex-1 flex-col p-8">
            <h1
              className="flex items-center text-[34px] leading-none font-bold tracking-[0.1em] text-foreground"
              aria-label="POMME"
            >
              <span>P</span>
              <img
                src="/pomme.png"
                alt="O"
                draggable={false}
                className="mx-0.5 size-[26px] select-none [image-rendering:pixelated]"
              />
              <span>MME</span>
            </h1>

            <div className="mt-auto flex max-w-[320px] flex-col gap-3 pt-6">
              <DropdownMenu.Root>
                <DropdownMenu.Trigger asChild>
                  <button className="group flex h-11 w-full min-w-0 items-center gap-2.5 rounded-xl border border-white/[0.08] bg-black/25 px-4 text-[13px] font-medium text-foreground shadow-[inset_0_1px_0_rgba(255,255,255,0.06)] backdrop-blur-xl transition-all duration-200 ease-out-soft hover:border-white/15 hover:bg-black/15 data-[state=open]:border-white/15 data-[state=open]:bg-black/15">
                    <Box size={ICON_SIZE} className="shrink-0 text-muted" />
                    <span className="min-w-0 flex-1 truncate text-left">
                      {activeInstall?.name || "No installation selected"}
                    </span>
                    <span
                      className="shrink-0 text-xs text-muted uppercase tabular-nums"
                      hidden={!activeInstall}
                    >
                      {activeInstall?.version || ""}
                    </span>
                    <ChevronDown
                      size={CHEVRON_SIZE}
                      className="shrink-0 text-muted transition-transform duration-200 group-data-[state=open]:rotate-180"
                    />
                  </button>
                </DropdownMenu.Trigger>
                <DropdownMenu.Portal>
                  <DropdownMenu.Content
                    align="start"
                    sideOffset={6}
                    className="menu w-(--radix-dropdown-menu-trigger-width)"
                  >
                    <div className="max-h-[240px] overflow-y-auto">
                      {hasInstallations ? (
                        <DropdownMenu.RadioGroup
                          value={activeInstall?.id}
                          onValueChange={selectInstallation}
                        >
                          {installations.map((installation) => (
                            <DropdownMenu.RadioItem
                              key={installation.id}
                              value={installation.id}
                              className="menu-item py-[9px]"
                            >
                              <span>{installation.name}</span>
                              <span className="text-xs text-muted uppercase tabular-nums">
                                {installation.version}
                              </span>
                            </DropdownMenu.RadioItem>
                          ))}
                        </DropdownMenu.RadioGroup>
                      ) : (
                        <DropdownMenu.Item
                          className="menu-item py-[9px]"
                          onSelect={openNewInstallationDialog}
                        >
                          <span>Create a new installation</span>
                        </DropdownMenu.Item>
                      )}
                    </div>
                  </DropdownMenu.Content>
                </DropdownMenu.Portal>
              </DropdownMenu.Root>
              <button
                className={`${playButtonClass} ${getPlayCursorClass(launchingStatus)}`}
                onClick={() => handleLaunch()}
                disabled={isBusy}
              >
                {showPlayIcon ? (
                  <Play size={PLAY_ICON_SIZE} fill="currentColor" />
                ) : (
                  <Download size={PLAY_ICON_SIZE} />
                )}
                <span
                  className={
                    isBusy
                      ? `text-sm font-semibold ${isLaunching ? "animate-pulse" : ""}`
                      : "text-[15px] font-bold tracking-[0.16em]"
                  }
                >
                  {playLabel}
                </span>
              </button>

              {downloadProgress && (
                <div className="flex w-full flex-col gap-2.5 pt-1">
                  <div className="text-xs text-muted">{downloadProgress.status}</div>
                  <div className="relative h-1 rounded-full bg-white/[0.08]">
                    <SkinRunner
                      skinUrl={skinUrl}
                      progress={getProgressFraction(downloadProgress)}
                    />
                    <div
                      className="h-full rounded-full bg-green transition-[width] duration-300"
                      style={{ width: `${getProgressFraction(downloadProgress) * 100}%` }}
                    />
                  </div>
                </div>
              )}
              {!downloadProgress && status && (
                <div className="pt-1 text-xs text-muted">{status}</div>
              )}
            </div>
          </div>

          <div className="relative flex w-[230px] shrink-0 items-end justify-center pb-5">
            <div className="pointer-events-none absolute bottom-10 h-3.5 w-24 rounded-full bg-black/70 blur-md" />
            {skinUrl ? (
              <SkinPreview skinUrl={skinUrl} pointer={pagePointer} />
            ) : (
              <img
                src="/pomme.png"
                alt=""
                draggable={false}
                className="relative mb-16 size-24 animate-float drop-shadow-[0_14px_22px_rgba(0,0,0,0.45)] select-none [image-rendering:pixelated]"
              />
            )}
          </div>
        </div>

        <div
          className="group relative min-h-[340px] cursor-pointer overflow-hidden rounded-2xl border border-white/[0.08] bg-panel shadow-[0_24px_48px_-24px_rgba(0,0,0,0.7)] transition-colors duration-300 hover:border-white/15"
          onClick={openFeatured}
          onPointerMove={(event) => setFeaturedPointer(getPointerOffset(event))}
          onPointerLeave={() => setFeaturedPointer(POINTER_CENTERED)}
        >
          {featured ? (
            <>
              <div className="absolute inset-0 animate-kenburns">
                <img
                  src={featured.image_url}
                  alt={featured.title}
                  className="absolute inset-0 size-full scale-110 object-cover transition-transform duration-700 ease-out-soft"
                  style={getParallaxStyle(featuredPointer)}
                />
              </div>
              <div className="absolute inset-0 bg-[linear-gradient(180deg,rgba(14,14,15,0.05)_25%,rgba(14,14,15,0.94)_100%),linear-gradient(90deg,transparent_50%,rgba(14,14,15,0.45))]" />
              <span className="news-badge top-4 left-4">{featured.entry_type}</span>
              <div className="absolute inset-x-0 bottom-0 flex flex-col gap-1.5 p-7">
                <span className="text-[11px] text-muted tabular-nums">
                  {formatDate(featured.date)}
                </span>
                <h2 className="text-[22px] leading-tight font-semibold text-foreground">
                  {featured.title}
                </h2>
                <p className="line-clamp-2 max-w-[60ch] text-xs leading-relaxed text-muted">
                  {featured.summary}
                </p>
              </div>
            </>
          ) : (
            <p className="empty-text">Loading patch notes...</p>
          )}
        </div>
      </section>

      {hasServers && (
        <section>
          <h2 className="label-caps mb-4">SERVERS</h2>
          <div className="glass grid grid-cols-[repeat(auto-fit,minmax(240px,1fr))] overflow-hidden rounded-2xl">
            {servers.map((server) => (
              <button key={server.id} className={serverRowClass} onClick={() => joinServer(server)}>
                <span
                  className={`size-1.5 shrink-0 rounded-full ${server.online ? "bg-green" : "bg-red"}`}
                />
                <span className="flex min-w-0 flex-1 flex-col gap-0.5">
                  <span className="truncate text-[13px] font-medium text-foreground">
                    {server.name}
                  </span>
                  <span className="truncate text-[11px] text-muted">{server.ip}</span>
                </span>
                <span className="flex flex-col items-end gap-0.5 text-xs tabular-nums">
                  <span className="text-foreground">{getPlayersText(server)}</span>
                  <span className="text-[11px] text-muted">{getPingText(server)}</span>
                </span>
                <span className="flex size-7 shrink-0 items-center justify-center rounded-lg bg-green/10 text-green opacity-0 transition-opacity duration-200 group-hover:opacity-100">
                  <Play size={JOIN_ICON_SIZE} fill="currentColor" />
                </span>
              </button>
            ))}
          </div>
        </section>
      )}

      {hasFriends && (
        <section>
          <h2 className="label-caps mb-4">FRIENDS</h2>
          <div className="grid grid-cols-[repeat(auto-fit,minmax(220px,1fr))] gap-3">
            {friends.map((friend) => {
              const presence = friendsPresence[friend.profileId];
              const offline = isOffline(presence);
              const friendSkin = friendsSkins[friend.profileId];
              return (
                <div key={friend.profileId} className={chipClass}>
                  <div
                    className={`skin-head ${offline ? "opacity-60 grayscale-[0.8]" : ""}`}
                    style={friendSkin ? { backgroundImage: `url("${friendSkin}")` } : undefined}
                  />
                  <span className="flex min-w-0 flex-1 flex-col gap-0.5">
                    <span
                      className={`truncate text-[13px] font-medium ${offline ? "text-muted" : "text-foreground"}`}
                    >
                      {friend.name}
                    </span>
                    <span className="truncate text-[11px] text-muted">
                      {formatStatus(presence)}
                    </span>
                  </span>
                  <span
                    className={`size-1.5 shrink-0 rounded-full ${offline ? "bg-line-strong" : "bg-green"}`}
                  />
                </div>
              );
            })}
          </div>
        </section>
      )}

      <section>
        <h2 className="label-caps mb-4">LATEST NEWS</h2>
        <div className="grid grid-cols-3 gap-5 @min-[960px]:grid-cols-4">
          {gridNews.map((item, index) => {
            const isWideOnly = index === WIDE_ONLY_CARD_INDEX;
            return (
              <div
                className={`${newsCardClass} ${isWideOnly ? "hidden @min-[960px]:block" : ""}`}
                style={{ animationDelay: `${index * NEWS_STAGGER_MS}ms` }}
                key={item.version}
                onClick={() => openPatchNote(item)}
              >
                <div className="relative aspect-video w-full overflow-hidden bg-panel">
                  <img
                    src={item.image_url}
                    alt={item.title}
                    className="absolute inset-0 size-full object-cover transition-transform duration-500 ease-out-soft group-hover:scale-[1.04]"
                  />
                  <span className="news-badge">{item.entry_type}</span>
                </div>

                <div className="flex flex-col gap-1.5 p-4">
                  <div className="flex items-center justify-between">
                    <span className="text-[11px] text-muted tabular-nums">
                      {formatDate(item.date)}
                    </span>
                    <span className="-translate-x-1 text-xs leading-none text-green opacity-0 transition-all duration-200 ease-out-soft group-hover:translate-x-0 group-hover:opacity-100">
                      →
                    </span>
                  </div>

                  <h3 className="text-sm leading-snug font-semibold text-foreground">
                    {item.title}
                  </h3>
                  <p className="line-clamp-2 text-xs leading-relaxed text-muted">{item.summary}</p>
                </div>
              </div>
            );
          })}
        </div>
      </section>
    </div>
  );
}
