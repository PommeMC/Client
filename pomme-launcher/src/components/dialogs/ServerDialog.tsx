import { ChevronDown } from "lucide-react";
import { Dialog, DropdownMenu } from "radix-ui";
import { useState } from "react";
import { useAppStore } from "../../lib/store";
import { Server } from "../../lib/types";

const UNCATEGORIZED = "Uncategorized";
const ENTER_KEY = "Enter";
const ICON_SIZE = 14;

type ServerCategoryInputProps = {
  category: string;
  setCategory: (category: string) => void;
  existingCategories: string[];
  customCategory: boolean;
  setCustomCategory: (custom: boolean) => void;
};

function ServerCategoryInput({
  category,
  setCategory,
  existingCategories,
  customCategory,
  setCustomCategory,
}: ServerCategoryInputProps) {
  const pickCategory = (picked: string) => {
    setCustomCategory(false);
    setCategory(picked);
  };

  const startCustomCategory = () => {
    setCustomCategory(true);
    setCategory("");
  };

  const chevron = (
    <ChevronDown
      size={ICON_SIZE}
      className="text-muted transition-transform duration-100 group-data-[state=open]:rotate-180"
    />
  );

  return (
    <div className="dialog-field">
      <label className="field-label">CATEGORY</label>
      <DropdownMenu.Root>
        {customCategory ? (
          <div className="field flex items-stretch p-0 focus-within:border-green">
            <input
              className="min-w-0 flex-1 bg-transparent px-3 py-[9px] text-[13px] text-foreground placeholder:text-faint"
              placeholder="New category name"
              value={category}
              onChange={(event) => setCategory(event.target.value)}
              autoFocus
            />
            <DropdownMenu.Trigger asChild>
              <button
                type="button"
                className="group flex w-9 shrink-0 items-center justify-center border-l border-white/[0.08]"
              >
                {chevron}
              </button>
            </DropdownMenu.Trigger>
          </div>
        ) : (
          <DropdownMenu.Trigger asChild>
            <button type="button" className="group field flex items-center justify-between">
              <span>{category}</span>
              {chevron}
            </button>
          </DropdownMenu.Trigger>
        )}
        <DropdownMenu.Portal>
          <DropdownMenu.Content
            align="end"
            sideOffset={4}
            className="menu w-(--radix-dropdown-menu-trigger-width) min-w-[220px]"
          >
            <DropdownMenu.RadioGroup
              value={category}
              onValueChange={pickCategory}
              className="max-h-[200px] overflow-y-auto"
            >
              <DropdownMenu.RadioItem value={UNCATEGORIZED} className="menu-item">
                <span>{UNCATEGORIZED}</span>
              </DropdownMenu.RadioItem>
              {existingCategories.map((existing) => (
                <DropdownMenu.RadioItem key={existing} value={existing} className="menu-item">
                  <span>{existing}</span>
                </DropdownMenu.RadioItem>
              ))}
            </DropdownMenu.RadioGroup>
            <DropdownMenu.Item className="menu-item" onSelect={startCustomCategory}>
              <span>+ New category</span>
            </DropdownMenu.Item>
          </DropdownMenu.Content>
        </DropdownMenu.Portal>
      </DropdownMenu.Root>
    </div>
  );
}

export type ServerDialogProps = { type: "new" } | { type: "edit"; server: Server };

export function ServerDialog(dialogProps: ServerDialogProps) {
  const servers = useAppStore((state) => state.servers);
  const addServer = useAppStore((state) => state.addServer);
  const editServer = useAppStore((state) => state.editServer);
  const setOpenedDialog = useAppStore((state) => state.setOpenedDialog);

  const isEdit = dialogProps.type === "edit";
  const editedServer = isEdit ? dialogProps.server : null;

  const [serverName, setServerName] = useState(editedServer?.name ?? "");
  const [serverAddress, setServerAddress] = useState(editedServer?.ip ?? "");
  const [category, setCategory] = useState(editedServer?.category || UNCATEGORIZED);
  const [customCategory, setCustomCategory] = useState(false);

  const existingCategories = [
    ...new Set(servers.map((server) => server.category).filter((name) => name)),
  ];

  const handleConfirm = () => {
    const ip = serverAddress.trim();
    const hasAddress = ip !== "";
    if (!hasAddress) {
      return;
    }

    const name = serverName.trim() || ip;
    const trimmedCategory = category.trim();
    const isUncategorized = trimmedCategory === UNCATEGORIZED;
    const savedCategory = isUncategorized ? "" : trimmedCategory;

    if (editedServer) {
      editServer(editedServer.id, name, ip, savedCategory);
    } else {
      addServer(name, ip, savedCategory);
    }

    setOpenedDialog(null);
  };

  const confirmOnEnter = (event: React.KeyboardEvent<HTMLInputElement>) => {
    const isEnter = event.key === ENTER_KEY;
    if (isEnter) {
      handleConfirm();
    }
  };

  return (
    <>
      <Dialog.Title className="dialog-title">{isEdit ? "Edit Server" : "Add Server"}</Dialog.Title>

      <div className="dialog-fields">
        <div className="dialog-field">
          <label className="field-label">SERVER NAME</label>
          <input
            className="field"
            value={serverName}
            onChange={(event) => setServerName(event.target.value)}
            placeholder="My Server"
            autoFocus
          />
        </div>

        <div className="dialog-field">
          <label className="field-label">SERVER ADDRESS</label>
          <input
            className="field"
            value={serverAddress}
            onChange={(event) => setServerAddress(event.target.value)}
            placeholder="play.example.com"
            onKeyDown={confirmOnEnter}
          />
        </div>

        <ServerCategoryInput
          category={category}
          setCategory={setCategory}
          customCategory={customCategory}
          setCustomCategory={setCustomCategory}
          existingCategories={existingCategories}
        />
      </div>

      <div className="dialog-actions">
        <button className="button-secondary" onClick={() => setOpenedDialog(null)}>
          Cancel
        </button>
        <button className="button-primary" onClick={handleConfirm}>
          {isEdit ? "Save" : "Add"}
        </button>
      </div>
    </>
  );
}
