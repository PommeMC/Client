import { AnsiHtml } from "fancy-ansi/react";
import { ClipboardCopy, X } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { commands, events } from "../bindings";
import Titlebar from "./Titlebar";

import { assertNever } from "../lib/helpers";
import "../styles.css";

const MAX_LOGS = 10_000;
const COPY_ICON_SIZE = 16;
const CLEAR_ICON_SIZE = 14;
const LOG_LEVELS = ["INFO", "WARN", "DEBUG", "ERROR"];
const LOG_CLASSES: Record<string, string> = {
  INFO: "log-info",
  WARN: "log-warn",
  DEBUG: "log-debug",
  ERROR: "log-error",
};

interface Filter {
  info_enabled: boolean;
  warn_enabled: boolean;
  debug_enabled: boolean;
  error_enabled: boolean;
  search?: string;
}

type LevelKey = "info_enabled" | "warn_enabled" | "debug_enabled" | "error_enabled";

const LEVEL_KEYS: Record<string, LevelKey> = {
  INFO: "info_enabled",
  WARN: "warn_enabled",
  DEBUG: "debug_enabled",
  ERROR: "error_enabled",
};

const FILTER_LEVELS: Array<{ key: LevelKey; label: string; activeClass: string }> = [
  { key: "info_enabled", label: "INFO", activeClass: "border-log-info text-log-info" },
  { key: "warn_enabled", label: "WARN", activeClass: "border-log-warning text-log-warning" },
  { key: "debug_enabled", label: "DEBUG", activeClass: "border-log-debug text-log-debug" },
  { key: "error_enabled", label: "ERROR", activeClass: "border-log-error text-log-error" },
];

const getLogs = async (): Promise<string[]> => {
  const result = await commands.getClientLogs();
  const succeeded = result.ok;
  if (!succeeded) {
    console.error("Error while getting client logs: ", result.error);
    return [];
  }
  return result.value;
};

function getLogLevel(log: string): string {
  const level = LOG_LEVELS.find((tag) => log.includes(tag)) ?? "";
  return level;
}

function isLogVisible(log: string, filter: Filter): boolean {
  const level = getLogLevel(log);
  const levelKey = LEVEL_KEYS[level] ?? "debug_enabled";
  const levelEnabled = filter[levelKey];
  const search = filter.search ?? "";
  const hasSearch = search !== "";
  const matchesSearch = !hasSearch || log.includes(search);
  const visible = levelEnabled && matchesSearch;
  return visible;
}

const Log = ({ log, filter }: { log: string; filter: Filter }) => {
  const visible = isLogVisible(log, filter);
  if (!visible) {
    return null;
  }
  const levelClass = LOG_CLASSES[getLogLevel(log)] ?? "";
  return (
    <p className="whitespace-pre">
      <AnsiHtml className={levelClass} text={log} />
    </p>
  );
};

export default function Console() {
  const [logs, setLogs] = useState<string[]>([]);
  const [filter, setFilter] = useState<Filter>({
    info_enabled: true,
    warn_enabled: true,
    debug_enabled: true,
    error_enabled: true,
  });

  const bottomRef = useRef<HTMLDivElement | null>(null);
  const searchRef = useRef<HTMLInputElement | null>(null);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let eventsRegistered = false;

    const initListener = async () => {
      const initialLogs = await getLogs();

      if (eventsRegistered) {
        return;
      }

      setLogs(initialLogs);

      const unlistenFn = await events.consoleMessageEvent.listen((event) => {
        const received = event.payload;
        switch (received.type) {
          case "message":
            setLogs((previous) => {
              const updated = [...previous, received.val];
              const overflowing = updated.length > MAX_LOGS;
              if (overflowing) {
                return updated.slice(1);
              }
              return updated;
            });
            break;
          case "reset":
            setLogs([]);
            break;
          default:
            assertNever(received);
        }
      });

      if (eventsRegistered) {
        unlistenFn();
        return;
      }

      unlisten = unlistenFn;
    };

    initListener();

    return () => {
      eventsRegistered = true;
      unlisten?.();
    };
  }, []);

  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [logs]);

  const copyLogs = () => {
    navigator.clipboard.writeText(logs.join("\n"));
  };

  const clearSearch = () => {
    const input = searchRef.current;
    const hasInput = input !== null;
    if (hasInput) {
      input.value = "";
    }
    setFilter((previous) => ({ ...previous, search: undefined }));
  };

  return (
    <div className="relative flex h-full flex-col overflow-hidden border border-line bg-background">
      <Titlebar name="Pomme Debugger" />
      <div className="flex min-h-0 flex-1 flex-col gap-2 p-2">
        <div className="min-h-0 flex-1 rounded-xl border border-white/[0.08] bg-black/30 p-3">
          <div className="h-full overflow-auto font-mono text-xs leading-normal select-text">
            {logs.map((log, index) => (
              <Log log={log} key={index} filter={filter} />
            ))}
            <div ref={bottomRef} />
          </div>
        </div>
        <div className="flex h-9 shrink-0 items-center gap-2">
          <button className="button-primary size-9 p-0" onClick={copyLogs}>
            <ClipboardCopy size={COPY_ICON_SIZE} />
          </button>
          {FILTER_LEVELS.map(({ key, label, activeClass }) => {
            const active = filter[key];
            return (
              <button
                key={key}
                onClick={() => setFilter((previous) => ({ ...previous, [key]: !previous[key] }))}
                className={`h-9 rounded-lg border px-3 text-[10px] font-semibold tracking-[0.1em] transition-colors duration-150 ${
                  active ? activeClass : "border-white/[0.08] bg-black/25 text-faint"
                }`}
              >
                {label}
              </button>
            );
          })}
          <input
            placeholder="Search..."
            type="text"
            ref={searchRef}
            className="field h-9 min-w-0 flex-1 py-0"
            onInput={(event) => {
              const value = event.currentTarget.value;
              setFilter((previous) => ({ ...previous, search: value }));
            }}
          />
          <button className="button-secondary button-icon size-9" onClick={clearSearch}>
            <X size={CLEAR_ICON_SIZE} />
          </button>
        </div>
      </div>
    </div>
  );
}
