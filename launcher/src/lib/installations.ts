import { useState } from "react";
import { Installation } from "./types.ts";
import { invoke } from "@tauri-apps/api/core";

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

  return {
    installations,
    setInstallations,
    invokeCreateInstallation,
    invokeDeleteInstallation,
    activeInstall,
    setActiveInstall,
    selectedInstall,
    setSelectedInstall,
  };
};
