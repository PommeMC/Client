import { invoke } from "@tauri-apps/api/core";
import { useState } from "react";
import { Installation } from "./types.ts";

export const useInstallations = () => {
  const [installations, setInstallations] = useState<Installation[]>([]);
  const [activeInstall, setActiveInstall] = useState<Installation | null>(null);
  const [selectedInstall, setSelectedInstall] = useState<Installation | null>(null);

  const invokeCreateInstallation = async (payload: Installation): Promise<Installation> => {
    return invoke<Installation>("create_installation", { payload });
  };

  const invokeDeleteInstallation = async (id: string): Promise<void> => {
    return invoke("delete_installation", { id });
  };

  const invokeDuplicateInstallation = async (
    original_id: string,
    install: Installation,
  ): Promise<Installation> => {
    return invoke<Installation>("duplicate_installation", {
      oldId: original_id,
      payload: install,
    });
  };

  const invokeEditInstallation = async (
    id: string,
    payload: Installation,
  ): Promise<Installation> => {
    return invoke("edit_installation", { id: id, payload: payload });
  };

  return {
    installations,
    setInstallations,
    invokeCreateInstallation,
    invokeDeleteInstallation,
    invokeDuplicateInstallation,
    invokeEditInstallation,
    activeInstall,
    setActiveInstall,
    selectedInstall,
    setSelectedInstall,
  };
};
