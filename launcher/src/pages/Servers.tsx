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
import { useState } from "react";
import { HiArrowPath, HiChevronDown, HiEllipsisVertical, HiPlay, HiPlus } from "react-icons/hi2";
import { useDropdown } from "../lib/hooks";
import { useAppStateContext } from "../lib/state";
import { Server } from "../lib/types";

const numFormatter = new Intl.NumberFormat();

function SortableServer({
  s,
  handleLaunch,
  startEdit,
  removeServer,
}: {
  s: Server;
  handleLaunch: (ip: string) => void;
  startEdit: (s: Server) => void;
  removeServer: (ip: string) => void;
}) {
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } = useSortable({
    id: s.id,
  });

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
          className={`server-menu-btn ${false ? "active" : ""}`}
          onPointerDown={(e) => e.stopPropagation()}
        >
          <HiEllipsisVertical />
        </button>
      </div>
    </div>
  );
}

export default function ServersPage({
  handleLaunch,
}: {
  handleLaunch: (ip: string) => Promise<void>;
}) {
  const { servers, addServer, editServer, moveServer, removeServer, pingAll } =
    useAppStateContext();
  const [addingServer, setAddingServer] = useState(false);
  const [editingId, setEditingId] = useState<string | null>(null);
  const [newName, setNewName] = useState("");
  const [newIp, setNewIp] = useState("");
  const [newCategory, setNewCategory] = useState("");
  const [customCategory, setCustomCategory] = useState(false);
  const { ref: categoryDropdownRef, ...categoryDropdown } = useDropdown();

  const sensors = useSensors(useSensor(PointerSensor, { activationConstraint: { distance: 5 } }));

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
    setEditingId(s.id);
    setNewName(s.name);
    setNewIp(s.ip);
    setNewCategory(s.category);
    setCustomCategory(false);
    setAddingServer(false);
  };

  const handleEdit = () => {
    if (editingId && newIp.trim()) {
      editServer(editingId, newName.trim() || newIp.trim(), newIp.trim(), newCategory.trim());
      setEditingId(null);
      setNewName("");
      setNewIp("");
      setNewCategory("");
    }
  };

  const cancelForm = () => {
    setAddingServer(false);
    setEditingId(null);
    setNewName("");
    setNewIp("");
    setNewCategory("");
    setCustomCategory(false);
    categoryDropdown.close();
  };

  const existingCategories = [...new Set(servers.map((s) => s.category).filter((c) => c))];
  const categories = [...new Set(servers.map((s) => s.category || ""))];
  const grouped: Record<string, Server[]> = {};
  for (const cat of categories) {
    grouped[cat] = servers.filter((s) => (s.category || "") === cat);
  }

  const showForm = addingServer || editingId !== null;

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
            onKeyDown={(e) => e.key === "Enter" && (editingId ? handleEdit() : handleAdd())}
          />
          {customCategory ? (
            <input
              className="servers-add-input"
              placeholder="New category name"
              value={newCategory}
              onChange={(e) => setNewCategory(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && (editingId ? handleEdit() : handleAdd())}
            />
          ) : (
            <div className="category-dropdown-wrapper" ref={categoryDropdownRef}>
              <button className="category-dropdown-btn" onClick={categoryDropdown.toggle}>
                <span>{newCategory || "No category"}</span>
                <HiChevronDown
                  className={`category-chevron ${categoryDropdown.isOpen ? "open" : ""}`}
                />
              </button>
              {categoryDropdown.isOpen && (
                <div className="category-dropdown">
                  <button
                    className={`category-dropdown-item ${!newCategory ? "active" : ""}`}
                    onClick={() => {
                      setNewCategory("");
                      categoryDropdown.close();
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
                        categoryDropdown.close();
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
                      categoryDropdown.close();
                    }}
                  >
                    + New category
                  </button>
                </div>
              )}
            </div>
          )}
          <button className="servers-add-confirm" onClick={editingId ? handleEdit : handleAdd}>
            {editingId ? "Save" : "Add"}
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
                    startEdit={startEdit}
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
