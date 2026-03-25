import React, { useState } from "react";
import {
  HiArrowPath,
  HiChevronDown,
  HiEllipsisVertical,
  HiPencil,
  HiPlay,
  HiPlus,
  HiTrash,
} from "react-icons/hi2";
import {
  DndContext,
  closestCenter,
  DragEndEvent,
  PointerSensor,
  useSensor,
  useSensors,
} from "@dnd-kit/core";
import { SortableContext, useSortable, rectSortingStrategy } from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import { restrictToWindowEdges } from "@dnd-kit/modifiers";
import { useAppStateContext } from "../lib/state";
import { Server } from "../lib/types";

function SortableServer({
  s,
  runGame,
  startEdit,
  removeServer,
  menuOpen,
  setMenuOpen,
  numFormatter,
}: {
  s: Server;
  runGame: (ip: string) => void;
  startEdit: (s: Server) => void;
  removeServer: (ip: string) => void;
  menuOpen: string | null;
  setMenuOpen: (ip: string | null) => void;
  numFormatter: Intl.NumberFormat;
}) {
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } = useSortable({
    id: s.ip,
  });

  const style = {
    transform: CSS.Transform.toString(transform),
    transition: isDragging ? "none" : transition,
    opacity: isDragging ? 0.4 : 1,
  };

  return (
    <div ref={setNodeRef} style={style} className="server" {...attributes} {...listeners}>
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
          onClick={() => runGame(s.ip)}
        >
          <HiPlay /> Join
        </button>
        <div className="server-menu-wrapper">
          <button
            className="server-menu-btn"
            onPointerDown={(e) => e.stopPropagation()}
            onClick={() => setMenuOpen(menuOpen === s.ip ? null : s.ip)}
          >
            <HiEllipsisVertical />
          </button>
          {menuOpen === s.ip && (
            <>
              <div className="click-away" onClick={() => setMenuOpen(null)} />
              <div className="server-menu">
                <button
                  onClick={() => {
                    startEdit(s);
                    setMenuOpen(null);
                  }}
                >
                  <HiPencil /> Edit
                </button>
                <button
                  className="server-menu-danger"
                  onClick={() => {
                    removeServer(s.ip);
                    setMenuOpen(null);
                  }}
                >
                  <HiTrash /> Delete
                </button>
              </div>
            </>
          )}
        </div>
      </div>
    </div>
  );
}

export default function ServersPage({ handleLaunch }: { handleLaunch: () => Promise<void> }) {
  const { setServer, servers, addServer, editServer, moveServer, removeServer, pingAll } =
    useAppStateContext();
  const [addingServer, setAddingServer] = useState(false);
  const [editingIp, setEditingIp] = useState<string | null>(null);
  const [menuOpen, setMenuOpen] = useState<string | null>(null);
  const [newName, setNewName] = useState("");
  const [newIp, setNewIp] = useState("");
  const [newCategory, setNewCategory] = useState("");
  const [customCategory, setCustomCategory] = useState(false);
  const [categoryDropdownOpen, setCategoryDropdownOpen] = useState(false);

  const numFormatter = new Intl.NumberFormat();
  const sensors = useSensors(useSensor(PointerSensor, { activationConstraint: { distance: 5 } }));

  const runGame = (ip: string) => {
    setServer(ip);
    handleLaunch();
  };

  const handleAdd = () => {
    if (newIp.trim()) {
      addServer(newName.trim() || newIp.trim(), newIp.trim(), newCategory.trim());
      setNewName("");
      setNewIp("");
      setNewCategory("");
      setAddingServer(false);
    }
  };

  const startEdit = (s: Server) => {
    setEditingIp(s.ip);
    setNewName(s.name);
    setNewIp(s.ip);
    setNewCategory(s.category);
    setCustomCategory(false);
    setAddingServer(false);
  };

  const handleEdit = () => {
    if (editingIp && newIp.trim()) {
      editServer(editingIp, newName.trim() || newIp.trim(), newIp.trim(), newCategory.trim());
      setEditingIp(null);
      setNewName("");
      setNewIp("");
      setNewCategory("");
    }
  };

  const cancelForm = () => {
    setAddingServer(false);
    setEditingIp(null);
    setNewName("");
    setNewIp("");
    setNewCategory("");
    setCustomCategory(false);
    setCategoryDropdownOpen(false);
  };

  const existingCategories = [...new Set(servers.map((s) => s.category).filter((c) => c))];
  const categories = [...new Set(servers.map((s) => s.category || ""))];
  const grouped: Record<string, Server[]> = {};
  for (const cat of categories) {
    grouped[cat] = servers.filter((s) => (s.category || "") === cat);
  }

  const showForm = addingServer || editingIp !== null;

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
            onClick={() => {
              cancelForm();
              setAddingServer(true);
            }}
          >
            <HiPlus /> Add Server
          </button>
        </div>
      </div>
      {showForm && (
        <div className="servers-add-form">
          <input
            className="servers-add-input"
            placeholder="Server Name"
            value={newName}
            onChange={(e) => setNewName(e.target.value)}
          />
          <input
            className="servers-add-input"
            placeholder="Server Address"
            value={newIp}
            onChange={(e) => setNewIp(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && (editingIp ? handleEdit() : handleAdd())}
          />
          {customCategory ? (
            <input
              className="servers-add-input"
              placeholder="New category name"
              value={newCategory}
              onChange={(e) => setNewCategory(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && (editingIp ? handleEdit() : handleAdd())}
            />
          ) : (
            <div className="category-dropdown-wrapper">
              {categoryDropdownOpen && (
                <div className="click-away" onClick={() => setCategoryDropdownOpen(false)} />
              )}
              <button
                className="category-dropdown-btn"
                onClick={() => setCategoryDropdownOpen(!categoryDropdownOpen)}
              >
                <span>{newCategory || "No category"}</span>
                <HiChevronDown
                  className={`category-chevron ${categoryDropdownOpen ? "open" : ""}`}
                />
              </button>
              {categoryDropdownOpen && (
                <div className="category-dropdown">
                  <button
                    className={`category-dropdown-item ${!newCategory ? "active" : ""}`}
                    onClick={() => {
                      setNewCategory("");
                      setCategoryDropdownOpen(false);
                    }}
                  >
                    No category
                  </button>
                  {existingCategories.map((cat) => (
                    <button
                      key={cat}
                      className={`category-dropdown-item ${newCategory === cat ? "active" : ""}`}
                      onClick={() => {
                        setNewCategory(cat);
                        setCategoryDropdownOpen(false);
                      }}
                    >
                      {cat}
                    </button>
                  ))}
                  <button
                    className="category-dropdown-item category-dropdown-new"
                    onClick={() => {
                      setCustomCategory(true);
                      setNewCategory("");
                      setCategoryDropdownOpen(false);
                    }}
                  >
                    + New category
                  </button>
                </div>
              )}
            </div>
          )}
          <button className="servers-add-confirm" onClick={editingIp ? handleEdit : handleAdd}>
            {editingIp ? "Save" : "Add"}
          </button>
          <button className="servers-add-cancel" onClick={cancelForm}>
            Cancel
          </button>
        </div>
      )}
      {servers.length === 0 && (
        <p className="servers-empty">No servers added. Click "Add Server" to get started.</p>
      )}
      <DndContext
        sensors={sensors}
        collisionDetection={closestCenter}
        modifiers={[restrictToWindowEdges]}
        onDragEnd={handleDragEnd}
      >
        <SortableContext items={servers.map((s) => s.ip)} strategy={rectSortingStrategy}>
          <div className="servers-grid">
            {servers.map((s, i) => {
              const cat = s.category || "";
              const prevCat = i > 0 ? servers[i - 1].category || "" : null;
              const showHeader = cat && cat !== prevCat;
              return (
                <React.Fragment key={s.ip}>
                  {showHeader && <h3 className="servers-category">{cat}</h3>}
                  <SortableServer
                    s={s}
                    runGame={runGame}
                    startEdit={startEdit}
                    removeServer={removeServer}
                    menuOpen={menuOpen}
                    setMenuOpen={setMenuOpen}
                    numFormatter={numFormatter}
                  />
                </React.Fragment>
              );
            })}
          </div>
        </SortableContext>
      </DndContext>
    </div>
  );
}
