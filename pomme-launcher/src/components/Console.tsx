import { AnsiHtml } from "fancy-ansi/react";
import { useEffect, useRef, useState } from "react";
import { HiClipboardCopy } from "react-icons/hi";
import { HiXMark } from "react-icons/hi2";
import { commands, events } from "../bindings";
import Titlebar from "./Titlebar";

import { assertNever } from "../lib/helpers";
import "../styles.css";

const getLogs = async (): Promise<string[]> => {
  const res = await commands.getClientLogs();
  if (res.ok) {
    return res.value;
  }
  console.error("Error while getting client logs: ", res.error);
  return [];
};

interface Filter {
  info_enabled: boolean;
  warn_enabled: boolean;
  debug_enabled: boolean;
  error_enabled: boolean;
  search?: string;
}

const Log = ({ log, filter }: { log: string; filter: Filter }) => {
  const type = ["INFO", "WARN", "DEBUG", "ERROR"].find((tag) => log.includes(`${tag}`)) ?? "";

  let render =
    (type === "INFO" && filter.info_enabled) ||
    (type === "WARN" && filter.warn_enabled) ||
    (type === "DEBUG" && filter.debug_enabled) ||
    (type === "ERROR" && filter.error_enabled) ||
    (type === "" && filter.debug_enabled); // treat unknown as debug

  if (filter.search && !log.includes(filter.search)) render = false;
  if (!render) return null;

  return (
    <p className="console-log">
      <AnsiHtml className={`console-text ${type.toLowerCase()}`} text={log} />
    </p>
  );
};

const FILTER_LEVELS = [
  { key: "info_enabled" as const, label: "INFO", type: "info" },
  { key: "warn_enabled" as const, label: "WARN", type: "warning" },
  { key: "debug_enabled" as const, label: "DEBUG", type: "debug" },
  { key: "error_enabled" as const, label: "ERROR", type: "error" },
];

const FilterChip = ({
  label,
  type,
  active,
  onToggle,
}: {
  label: string;
  type: string;
  active: boolean;
  onToggle: () => void;
}) => (
  <button onClick={onToggle} className={`console-chip-button ${type} ${active ? "active" : ""}`}>
    {label}
  </button>
);

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

      if (eventsRegistered) return;

      setLogs(initialLogs);

      const unlistenFn = await events.consoleMessageEvent.listen((event) => {
        let recv = event.payload;
        switch (recv.type) {
          case "message":
            setLogs((prev) => {
              const updated = [...prev, recv.val];
              const maxLogs = 10_000;
              if (updated.length > maxLogs) return updated.slice(1);
              return updated;
            });
            break;
          case "reset":
            setLogs([]);
            break;
          default:
            assertNever(recv);
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

  return (
    <div className="app">
      <Titlebar name="Pomme Debugger" />
      <div className="console-holder">
        <div className="console">
          <div className="console-scroll">
            {logs.map((log, key) => (
              <Log log={log} key={key} filter={filter} />
            ))}
            <div ref={bottomRef} />
          </div>
        </div>
        <div className="console-bottom-bar">
          <button className="console-copy-button" onClick={copyLogs}>
            <HiClipboardCopy className="clipboard-icon" />
          </button>
          {FILTER_LEVELS.map(({ key, label, type }) => (
            <FilterChip
              key={key}
              label={label}
              type={type}
              active={filter[key]}
              onToggle={() => setFilter((prev) => ({ ...prev, [key]: !prev[key] }))}
            />
          ))}
          <input
            placeholder="Search..."
            type="text"
            ref={searchRef}
            className="console-search"
            onInput={(e) => {
              const value = e.currentTarget.value;
              setFilter((prev) => ({ ...prev, search: value }));
            }}
          />
          <button
            className="console-clear-search-button"
            onClick={() => {
              if (searchRef.current) {
                searchRef.current.value = "";
              }
              setFilter((prev) => ({ ...prev, search: undefined }));
            }}
          >
            <HiXMark />
          </button>
        </div>
      </div>
    </div>
  );
}
