import { useState } from "react";
import { Installation } from "../bindings/pomme_launcher/installations";

export const useInstallations = () => {
  const [installations, setInstallations] = useState<Installation[]>([]);
  const [activeInstall, setActiveInstall] = useState<Installation | null>(null);
  const [selectedInstall, setSelectedInstall] = useState<Installation | null>(null);

  return {
    installations,
    setInstallations,
    activeInstall,
    setActiveInstall,
    selectedInstall,
    setSelectedInstall,
  };
};
