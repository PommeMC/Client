import { useEffect, useRef, useState } from "react";

export function useDropdown() {
  const [isOpen, setIsOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!isOpen) return;
    let startedInside = false;
    const onMouseDown = (e: MouseEvent) => {
      startedInside = ref.current?.contains(e.target as Node) ?? false;
    };
    const onMouseUp = (e: MouseEvent) => {
      if (!startedInside && !ref.current?.contains(e.target as Node)) {
        setIsOpen(false);
      }
    };
    document.addEventListener("mousedown", onMouseDown);
    document.addEventListener("mouseup", onMouseUp);
    return () => {
      document.removeEventListener("mousedown", onMouseDown);
      document.removeEventListener("mouseup", onMouseUp);
    };
  }, [isOpen]);

  return {
    ref,
    isOpen,
    setIsOpen,
    toggle: () => setIsOpen((v) => !v),
    close: () => setIsOpen(false),
    open: () => setIsOpen(true),
  };
}
