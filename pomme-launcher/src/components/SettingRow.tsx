import { ReactNode } from "react";

interface SettingRowProps {
  label: string;
  desc: string;
  children: ReactNode;
}

export default function SettingRow({ label, desc, children }: SettingRowProps) {
  return (
    <div className="flex items-center justify-between gap-4 border-t border-line py-3.5 last:border-b">
      <div className="flex flex-col gap-[3px]">
        <span className="text-sm font-medium text-foreground">{label}</span>
        <span className="text-xs text-muted">{desc}</span>
      </div>
      <div className="shrink-0">{children}</div>
    </div>
  );
}
