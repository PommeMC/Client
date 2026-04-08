import { useState } from "react";
import { HiChevronDown } from "react-icons/hi2";
import { useDropdown } from "../../lib/hooks.ts";
import { useAppStateContext } from "../../lib/state.ts";
import { Server } from "../../lib/types.ts";

export type ServerDialogProps = { type: "new" } | { type: "edit"; server: Server };

export function ServerDialog(dialogProps: ServerDialogProps) {
  const { servers, addServer, editServer, setOpenedDialog } = useAppStateContext();

  const [serverName, setServerName] = useState(
    dialogProps.type === "edit" ? dialogProps.server.name : "",
  );
  const [serverAddress, setServerAddress] = useState(
    dialogProps.type === "edit" ? dialogProps.server.ip : "",
  );
  const [category, setCategory] = useState(
    dialogProps.type === "edit" ? dialogProps.server.category : "",
  );
  const [customCategory, setCustomCategory] = useState(false);

  const { ref: categoryDropdownRef, ...categoryDropdown } = useDropdown();

  const existingCategories = [...new Set(servers.map((s) => s.category).filter((c) => c))];

  const handleConfirm = () => {
    if (!serverAddress.trim()) return;

    const name = serverName.trim() || serverAddress.trim();
    const ip = serverAddress.trim();
    const cat = category.trim();

    if (dialogProps.type === "new") {
      addServer(name, ip, cat);
    } else {
      editServer(dialogProps.server.id, name, ip, cat);
    }

    setOpenedDialog(null);
  };

  return (
    <div
      className="dialog"
      onClick={(e) => {
        e.stopPropagation();
        if (categoryDropdown.isOpen) categoryDropdown.close();
      }}
    >
      <h2 className="dialog-title">{dialogProps.type === "edit" ? "Edit Server" : "Add Server"}</h2>

      <div className="dialog-fields">
        <div className="dialog-field">
          <label>SERVER NAME</label>
          <input
            value={serverName}
            onChange={(e) => setServerName(e.target.value)}
            placeholder="My Server"
            autoFocus
          />
        </div>

        <div className="dialog-field">
          <label>SERVER ADDRESS</label>
          <input
            value={serverAddress}
            onChange={(e) => setServerAddress(e.target.value)}
            placeholder="play.example.com"
            onKeyDown={(e) => e.key === "Enter" && handleConfirm()}
          />
        </div>

        <div className="dialog-field">
          <label>CATEGORY</label>
          {customCategory ? (
            <input
              placeholder="New category name"
              value={category}
              onChange={(e) => setCategory(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && handleConfirm()}
              autoFocus
            />
          ) : (
            <div className="custom-select-wrapper" ref={categoryDropdownRef}>
              <button className="custom-select" onClick={categoryDropdown.toggle} type="button">
                <span>{category || "No category"}</span>
                <HiChevronDown
                  className={`custom-select-arrow ${categoryDropdown.isOpen ? "open" : ""}`}
                />
              </button>
              {categoryDropdown.isOpen && (
                <div className="custom-select-dropdown">
                  <div className="custom-select-list">
                    <button
                      className={`custom-select-item ${!category ? "active" : ""}`}
                      onClick={() => {
                        setCategory("");
                        categoryDropdown.close();
                      }}
                    >
                      <span>No category</span>
                    </button>
                    {existingCategories.map((cat) => (
                      <button
                        key={cat}
                        className={`custom-select-item ${category === cat ? "active" : ""}`}
                        onClick={() => {
                          setCategory(cat);
                          categoryDropdown.close();
                        }}
                      >
                        <span>{cat}</span>
                      </button>
                    ))}
                    <button
                      className="custom-select-item"
                      onClick={() => {
                        setCustomCategory(true);
                        setCategory("");
                        categoryDropdown.close();
                      }}
                    >
                      <span>+ New category</span>
                    </button>
                  </div>
                </div>
              )}
            </div>
          )}
        </div>
      </div>

      <div className="dialog-actions">
        <button className="dialog-cancel" onClick={() => setOpenedDialog(null)}>
          Cancel
        </button>
        <button className="dialog-save" onClick={handleConfirm}>
          {dialogProps.type === "edit" ? "Save" : "Add"}
        </button>
      </div>
    </div>
  );
}
