import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Server, ServerStatus } from "./types";

const PING_INTERVAL_MS = 30_000;

interface SavedServer {
  name: string;
  address: string;
  category?: string;
}

export const useServers = () => {
  const [servers, setServers] = useState<Server[]>([]);
  const [loaded, setLoaded] = useState(false);

  useEffect(() => {
    invoke<SavedServer[]>("load_servers").then((saved) => {
      setServers(
        saved.map((s) => ({
          name: s.name,
          ip: s.address,
          category: s.category || "",
          players: 0,
          max_players: 0,
          ping: -1,
          online: false,
          motd: "",
          version: "",
        })),
      );
      setLoaded(true);
    });
  }, []);

  const persist = useCallback((list: Server[]) => {
    const saved: SavedServer[] = list.map((s) => ({
      name: s.name,
      address: s.ip,
      category: s.category || undefined,
    }));
    invoke("save_servers", { servers: saved });
  }, []);

  const pingOne = useCallback(async (ip: string) => {
    try {
      const status = await invoke<ServerStatus>("ping_server", { address: ip });
      setServers((prev) =>
        prev.map((s) =>
          s.ip === ip
            ? {
                ...s,
                online: status.online,
                players: status.players,
                max_players: status.max_players,
                ping: status.ping_ms,
                motd: status.motd,
                version: status.version,
              }
            : s,
        ),
      );
    } catch {
      setServers((prev) => prev.map((s) => (s.ip === ip ? { ...s, online: false, ping: -1 } : s)));
    }
  }, []);

  const pingAll = useCallback(() => {
    setServers((prev) => {
      for (const s of prev) {
        pingOne(s.ip);
      }
      return prev;
    });
  }, [pingOne]);

  useEffect(() => {
    if (!loaded || servers.length === 0) return;
    pingAll();
    const interval = setInterval(pingAll, PING_INTERVAL_MS);
    return () => clearInterval(interval);
  }, [loaded, servers.length, pingAll]);

  const addServer = (name: string, ip: string, category = "") => {
    const server: Server = {
      name,
      ip,
      category,
      players: 0,
      max_players: 0,
      ping: -1,
      online: false,
      motd: "",
      version: "",
    };
    setServers((prev) => {
      const next = [...prev, server];
      persist(next);
      return next;
    });
    pingOne(ip);
  };

  const editServer = (oldIp: string, name: string, ip: string, category: string) => {
    setServers((prev) => {
      const next = prev.map((s) => (s.ip === oldIp ? { ...s, name, ip, category } : s));
      persist(next);
      return next;
    });
    if (oldIp !== ip) {
      pingOne(ip);
    }
  };

  const moveServer = (fromIp: string, toIp: string) => {
    setServers((prev) => {
      const fromIdx = prev.findIndex((s) => s.ip === fromIp);
      const toIdx = prev.findIndex((s) => s.ip === toIp);
      if (fromIdx === -1 || toIdx === -1 || fromIdx === toIdx) return prev;
      const next = [...prev];
      const [moved] = next.splice(fromIdx, 1);
      const targetCat = prev[toIdx].category;
      moved.category = targetCat;
      next.splice(toIdx, 0, moved);
      persist(next);
      return next;
    });
  };

  const removeServer = (ip: string) => {
    setServers((prev) => {
      const next = prev.filter((s) => s.ip !== ip);
      persist(next);
      return next;
    });
  };

  return { servers, setServers, addServer, editServer, moveServer, removeServer, pingAll };
};
