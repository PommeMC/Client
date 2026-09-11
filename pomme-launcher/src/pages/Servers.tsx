import {
  closestCenter,
  DndContext,
  DragEndEvent,
  PointerSensor,
  useSensor,
  useSensors,
} from "@dnd-kit/core";
import { restrictToWindowEdges } from "@dnd-kit/modifiers";
import { rectSortingStrategy, SortableContext, useSortable } from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import { EllipsisVertical, Pencil, Play, Plus, RefreshCw, Trash2 } from "lucide-react";
import { DropdownMenu } from "radix-ui";
import { SyntheticEvent, useState } from "react";
import { getPingText, getPlayersText } from "../lib/servers";
import { useAppStore } from "../lib/store";
import { handleLaunchType, Server } from "../lib/types";

const GOOD_PING_MS = 100;
const OK_PING_MS = 200;
const DRAG_START_DISTANCE_PX = 5;
const UNCATEGORIZED_KEY = "__uncategorized";
const ICON_SIZE = 14;
const BUTTON_ICON_SIZE = 12;
const MENU_ICON_SIZE = 16;
const PING_OFFLINE_CLASS = "text-faint";
const PING_GOOD_CLASS = "text-green";
const PING_OK_CLASS = "text-muted";
const PING_BAD_CLASS = "text-red";

function getPingClass(ping: number): string {
  const isOffline = ping < 0;
  if (isOffline) {
    return PING_OFFLINE_CLASS;
  }
  const isGood = ping < GOOD_PING_MS;
  if (isGood) {
    return PING_GOOD_CLASS;
  }
  const isOk = ping < OK_PING_MS;
  if (isOk) {
    return PING_OK_CLASS;
  }
  return PING_BAD_CLASS;
}

function compareCategories(first: string, second: string): number {
  const firstIsDefault = first === "";
  if (firstIsDefault) {
    return -1;
  }
  const secondIsDefault = second === "";
  if (secondIsDefault) {
    return 1;
  }
  const order = first.localeCompare(second);
  return order;
}

function getSortedCategories(servers: Server[]): string[] {
  const unique = [...new Set(servers.map((server) => server.category || ""))];
  const sorted = unique.sort(compareCategories);
  return sorted;
}

function getServersInCategory(servers: Server[], category: string): Server[] {
  const inCategory = servers.filter((server) => (server.category || "") === category);
  return inCategory;
}

function stopDrag(event: SyntheticEvent) {
  event.stopPropagation();
}

interface ServerRowProps {
  server: Server;
  handleLaunch: handleLaunchType;
  startEdit: (server: Server) => void;
  removeServer: (id: string) => void;
}

function ServerRow({ server, handleLaunch, startEdit, removeServer }: ServerRowProps) {
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } = useSortable({
    id: server.id,
  });
  const setPage = useAppStore((state) => state.setPage);

  const style = {
    transform: CSS.Transform.toString(transform),
    transition: isDragging ? "none" : transition,
  };

  const join = () => {
    setPage("home");
    handleLaunch({ serverIp: server.ip, serverVersion: server.version });
  };

  return (
    <div
      ref={setNodeRef}
      style={style}
      className={`row cursor-grab gap-3 px-3.5 py-3 select-none ${
        isDragging ? "z-10 bg-panel opacity-70" : ""
      }`}
      {...attributes}
      {...listeners}
    >
      <div className={`size-1.5 shrink-0 ${server.online ? "bg-green" : "bg-red"}`} />
      <div className="flex min-w-0 flex-1 flex-col gap-0.5">
        <span className="text-[13px] font-medium text-foreground">{server.name}</span>
        <span className="text-xs text-muted">{server.ip}</span>
      </div>
      <span className="shrink-0 text-xs text-muted tabular-nums">{getPlayersText(server)}</span>
      <span
        className={`min-w-12 shrink-0 text-right text-xs font-medium tabular-nums ${getPingClass(server.ping)}`}
      >
        {getPingText(server)}
      </span>
      <button className="button-primary" onPointerDown={stopDrag} onClick={join}>
        <Play size={BUTTON_ICON_SIZE} fill="currentColor" /> Join
      </button>
      <DropdownMenu.Root>
        <DropdownMenu.Trigger asChild>
          <button
            className="flex size-7 items-center justify-center text-faint transition-colors duration-75 hover:text-foreground data-[state=open]:text-foreground"
            onPointerDown={stopDrag}
            onMouseDown={stopDrag}
            onMouseUp={stopDrag}
          >
            <EllipsisVertical size={MENU_ICON_SIZE} />
          </button>
        </DropdownMenu.Trigger>
        <DropdownMenu.Portal>
          <DropdownMenu.Content align="end" sideOffset={4} className="menu min-w-[140px] p-1">
            <DropdownMenu.Item
              className="menu-item justify-start gap-2 px-2.5"
              onSelect={() => startEdit(server)}
            >
              <Pencil size={ICON_SIZE} /> Edit
            </DropdownMenu.Item>
            <DropdownMenu.Separator className="my-1 h-px bg-line" />
            <DropdownMenu.Item
              className="menu-item justify-start gap-2 px-2.5 data-highlighted:text-red"
              onSelect={() => removeServer(server.id)}
            >
              <Trash2 size={ICON_SIZE} /> Remove
            </DropdownMenu.Item>
          </DropdownMenu.Content>
        </DropdownMenu.Portal>
      </DropdownMenu.Root>
    </div>
  );
}

export default function ServersPage({ handleLaunch }: { handleLaunch: handleLaunchType }) {
  const servers = useAppStore((state) => state.servers);
  const moveServer = useAppStore((state) => state.moveServer);
  const removeServer = useAppStore((state) => state.removeServer);
  const pingAllServers = useAppStore((state) => state.pingAllServers);
  const setOpenedDialog = useAppStore((state) => state.setOpenedDialog);

  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: DRAG_START_DISTANCE_PX } }),
  );

  const categories = getSortedCategories(servers);
  const hasServers = servers.length > 0;

  const handleDragEnd = (event: DragEndEvent) => {
    const { active, over } = event;
    const hasTarget = over !== null;
    if (!hasTarget) {
      return;
    }
    const samePlace = active.id === over.id;
    if (samePlace) {
      return;
    }
    moveServer(String(active.id), String(over.id));
  };

  const [spinning, setSpinning] = useState(false);

  const refresh = () => {
    setSpinning(true);
    pingAllServers();
  };

  const openAddDialog = () => {
    setOpenedDialog({ name: "server_dialog", props: { type: "new" } });
  };

  const openEditDialog = (server: Server) => {
    setOpenedDialog({ name: "server_dialog", props: { type: "edit", server } });
  };

  return (
    <div className="page relative">
      <div className="page-header">
        <h2 className="page-heading">SERVERS</h2>
        <div className="flex items-center gap-2">
          <button
            className={`button-secondary button-icon ${spinning ? "[&>svg]:animate-spin-once" : ""}`}
            onClick={refresh}
            onAnimationEnd={() => setSpinning(false)}
          >
            <RefreshCw size={ICON_SIZE} />
          </button>
          <button className="button-primary" onClick={openAddDialog}>
            <Plus size={ICON_SIZE} /> Add Server
          </button>
        </div>
      </div>

      {!hasServers && (
        <p className="empty-text">No servers added. Click "Add Server" to get started.</p>
      )}

      <DndContext
        sensors={sensors}
        collisionDetection={closestCenter}
        modifiers={[restrictToWindowEdges]}
        onDragEnd={handleDragEnd}
      >
        <SortableContext items={servers.map((server) => server.id)} strategy={rectSortingStrategy}>
          {categories.map((category) => (
            <div key={category || UNCATEGORIZED_KEY}>
              {category && <h3 className="label-caps mt-6 mb-2.5 uppercase">{category}</h3>}
              <div className="list">
                {getServersInCategory(servers, category).map((server) => (
                  <ServerRow
                    key={server.id}
                    server={server}
                    handleLaunch={handleLaunch}
                    startEdit={openEditDialog}
                    removeServer={removeServer}
                  />
                ))}
              </div>
            </div>
          ))}
        </SortableContext>
      </DndContext>
    </div>
  );
}
