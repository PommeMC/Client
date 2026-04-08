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
import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import {
  HiArrowPath,
  HiEllipsisVertical,
  HiPencil,
  HiPlay,
  HiPlus,
  HiTrash,
} from "react-icons/hi2";
import { useDropdown } from "../lib/hooks";
import { useAppStateContext } from "../lib/state";
import { Server } from "../lib/types";

const numFormatter = new Intl.NumberFormat();

function ServerMenu({
  anchorRef,
  menuRef,
  onEdit,
  onRemove,
  onClose,
}: {
  anchorRef: React.RefObject<HTMLButtonElement | null>;
  menuRef: React.RefObject<HTMLDivElement | null>;
  onEdit: () => void;
  onRemove: () => void;
  onClose: () => void;
}) {
  const [pos, setPos] = useState({ top: 0, left: 0 });

  useEffect(() => {
    if (anchorRef.current) {
      const rect = anchorRef.current.getBoundingClientRect();
      setPos({ top: rect.bottom + 4, left: rect.right - 120 });
    }
  }, []);

  return createPortal(
    <div
      ref={menuRef}
      className="server-menu"
      style={{ position: "fixed", top: pos.top, left: pos.left, zIndex: 1000 }}
      onPointerDown={(e) => e.stopPropagation()}
    >
      <button
        onClick={() => {
          onEdit();
          onClose();
        }}
      >
        <HiPencil /> Edit
      </button>
      <div className="server-menu-divider" />
      <button
        className="server-menu-danger"
        onClick={() => {
          onRemove();
          onClose();
        }}
      >
        <HiTrash /> Remove
      </button>
    </div>,
    document.body,
  );
}

function SortableServer({
  s,
  handleLaunch,
  startEdit,
  removeServer,
}: {
  s: Server;
  handleLaunch: (ip: string) => void;
  startEdit: (s: Server) => void;
  removeServer: (id: string) => void;
}) {
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } = useSortable({
    id: s.id,
  });
  const menuBtnRef = useRef<HTMLButtonElement>(null);
  const wasOpenRef = useRef(false);
  const { ref: menuRef, isOpen: menuOpen, open: openMenu, close: closeMenu } = useDropdown();

  const style = {
    transform: CSS.Transform.toString(transform),
    transition: isDragging ? "none" : transition,
  };

  return (
    <div
      ref={setNodeRef}
      style={style}
      className={`server ${isDragging ? "dragging" : ""}`}
      {...attributes}
      {...listeners}
    >
      <div className="server-top">
        <div className="server-status">
          <div className={`dot ${s.online ? "on" : "off"}`} />
        </div>
        <div className="server-info">
          <span className="server-name">{s.name}</span>
          <span className="server-ip">{s.ip}</span>
        </div>
        <span className="server-players">
          {s.online
            ? `${numFormatter.format(s.players)}/${numFormatter.format(s.max_players)}`
            : "—"}
        </span>
        <span className="server-ping">
          {s.ping >= 0 ? `${numFormatter.format(s.ping)}ms` : "—"}
        </span>
        <button
          className="install-play-btn"
          onPointerDown={(e) => e.stopPropagation()}
          onClick={() => handleLaunch(s.ip)}
        >
          <HiPlay /> Join
        </button>
        <button
          ref={menuBtnRef}
          className={`server-menu-btn ${menuOpen ? "active" : ""}`}
          onPointerDown={(e) => {
            e.stopPropagation();
            wasOpenRef.current = menuOpen;
          }}
          onClick={() => {
            if (wasOpenRef.current) {
              closeMenu();
            } else {
              openMenu();
            }
          }}
        >
          <HiEllipsisVertical />
        </button>
        {menuOpen && (
          <ServerMenu
            anchorRef={menuBtnRef}
            menuRef={menuRef}
            onEdit={() => startEdit(s)}
            onRemove={() => removeServer(s.id)}
            onClose={closeMenu}
          />
        )}
      </div>
    </div>
  );
}

export default function ServersPage({
  handleLaunch,
}: {
  handleLaunch: (ip: string) => Promise<void>;
}) {
  const { servers, moveServer, removeServer, pingAll, setOpenedDialog } = useAppStateContext();

  const sensors = useSensors(useSensor(PointerSensor, { activationConstraint: { distance: 5 } }));

  const categories = [...new Set(servers.map((s) => s.category || ""))];
  const grouped: Record<string, Server[]> = {};
  for (const cat of categories) {
    grouped[cat] = servers.filter((s) => (s.category || "") === cat);
  }

  const handleDragEnd = (event: DragEndEvent) => {
    const { active, over } = event;
    if (over && active.id !== over.id) {
      moveServer(active.id as string, over.id as string);
    }
  };

  return (
    <div className="page servers-page">
      <div className="servers-header">
        <h2 className="servers-heading">SERVERS</h2>
        <div className="servers-actions">
          <button className="servers-refresh-btn" onClick={pingAll}>
            <HiArrowPath />
          </button>
          <button
            className="servers-add-btn"
            onClick={() => setOpenedDialog({ name: "server_dialog", props: { type: "new" } })}
          >
            <HiPlus /> Add Server
          </button>
        </div>
      </div>

      {servers.length === 0 && (
        <p className="servers-empty">No servers added. Click "Add Server" to get started.</p>
      )}

      <DndContext
        sensors={sensors}
        collisionDetection={closestCenter}
        modifiers={[restrictToWindowEdges]}
        onDragEnd={handleDragEnd}
      >
        <SortableContext items={servers.map((s) => s.id)} strategy={rectSortingStrategy}>
          {categories.map((cat) => (
            <div key={cat || "__uncategorized"}>
              {cat && <h3 className="servers-category">{cat}</h3>}
              <div className="servers-grid">
                {grouped[cat].map((s) => (
                  <SortableServer
                    key={s.id}
                    s={s}
                    handleLaunch={handleLaunch}
                    startEdit={(s) =>
                      setOpenedDialog({ name: "server_dialog", props: { type: "edit", server: s } })
                    }
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
