import { useCallback, useEffect, useState } from "react";
import { commands } from "../bindings";
import { SavedServer } from "../bindings/pomme_launcher/ping";
import { Server } from "./types";

const PING_INTERVAL_MS = 30_000;

export const useServers = () => {
  const [servers, setServers] = useState<Server[]>([]);
  const [loaded, setLoaded] = useState(false);

  const persist = useCallback((list: Server[]) => {
    const saved: SavedServer[] = list.map((s) => ({
      name: s.name,
      address: s.ip,
      category: s.category || undefined,
    }));
    commands.saveServers(saved).then((res) => {
      if (!res.ok) console.error(res.error);
    });
  }, []);

  const pingOne = useCallback(async (id: string, ip: string) => {
    const status = await commands.pingServer(ip);
    setServers((prev) =>
      prev.map((s) =>
        s.id === id
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
  }, []);

  const pingAll = useCallback(() => {
    setServers((prev) => {
      for (const s of prev) {
        pingOne(s.id, s.ip);
      }
      return prev;
    });
  }, [pingOne]);

  useEffect(() => {
    commands.loadServers().then((saved) => {
      const list: Server[] = saved.map((s) => ({
        id: crypto.randomUUID(),
        name: s.name,
        ip: s.address,
        category: s.category || "",
        players: 0,
        max_players: 0,
        ping: -1,
        online: false,
        motd: "",
        version: "",
      }));
      setServers(list);
      setLoaded(true);
      for (const s of list) {
        pingOne(s.id, s.ip);
      }
    });
  }, [pingOne]);

  useEffect(() => {
    if (!loaded || servers.length === 0) return;
    const interval = setInterval(pingAll, PING_INTERVAL_MS);
    return () => clearInterval(interval);
  }, [loaded, servers.length, pingAll]);

  const addServer = (name: string, ip: string, category = "") => {
    const server: Server = {
      id: crypto.randomUUID(),
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
    pingOne(server.id, ip);
  };

  const editServer = (id: string, name: string, ip: string, category: string) => {
    let ipChanged = false;
    setServers((prev) => {
      const existing = prev.find((s) => s.id === id);
      ipChanged = existing ? existing.ip !== ip : false;
      const next = prev.map((s) => (s.id === id ? { ...s, name, ip, category } : s));
      persist(next);
      return next;
    });
    if (ipChanged) {
      pingOne(id, ip);
    }
  };

  const moveServer = (fromId: string, toId: string) => {
    setServers((prev) => {
      const fromIdx = prev.findIndex((s) => s.id === fromId);
      const toIdx = prev.findIndex((s) => s.id === toId);
      if (fromIdx === -1 || toIdx === -1 || fromIdx === toIdx) return prev;
      const next = [...prev];
      const [moved] = next.splice(fromIdx, 1);
      moved.category = prev[toIdx].category;
      next.splice(toIdx, 0, moved);
      persist(next);
      return next;
    });
  };

  const removeServer = (id: string) => {
    setServers((prev) => {
      const next = prev.filter((s) => s.id !== id);
      persist(next);
      return next;
    });
  };

  return { servers, setServers, addServer, editServer, moveServer, removeServer, pingAll };
};
