import { StateCreator } from "zustand";
import { commands } from "../bindings";
import { SavedServer } from "../bindings/pomme_launcher/ping";
import type { AppStore } from "./store";
import { Server } from "./types";

export const PING_INTERVAL_MS = 30_000;
const UNKNOWN_PING = -1;
const numberFormatter = new Intl.NumberFormat();

export function getPingText(server: Server): string {
  const isReachable = server.ping >= 0;
  if (!isReachable) {
    const offlineText = "offline";
    return offlineText;
  }
  const pingText = `${numberFormatter.format(server.ping)}ms`;
  return pingText;
}

export function getPlayersText(server: Server): string {
  if (!server.online) {
    const noPlayers = "—";
    return noPlayers;
  }
  const players = numberFormatter.format(server.players);
  const maxPlayers = numberFormatter.format(server.max_players);
  const playersText = `${players}/${maxPlayers}`;
  return playersText;
}

export type ServersSlice = {
  servers: Server[];
  serversLoaded: boolean;
  loadServers: () => Promise<void>;
  pingServer: (id: string, ip: string) => Promise<void>;
  pingAllServers: () => void;
  addServer: (name: string, ip: string, category: string) => void;
  editServer: (id: string, name: string, ip: string, category: string) => void;
  moveServer: (fromId: string, toId: string) => void;
  removeServer: (id: string) => void;
};

function createServer(name: string, ip: string, category: string): Server {
  const server: Server = {
    id: crypto.randomUUID(),
    name,
    ip,
    category,
    players: 0,
    max_players: 0,
    ping: UNKNOWN_PING,
    online: false,
    motd: "",
    version: "",
  };
  return server;
}

function saveServers(servers: Server[]) {
  const saved: SavedServer[] = servers.map((server) => ({
    name: server.name,
    address: server.ip,
    category: server.category || undefined,
    protocol: server.protocol,
  }));
  commands.saveServers(saved).then((result) => {
    const succeeded = result.ok;
    if (!succeeded) {
      console.error(result.error);
    }
  });
}

export const createServersSlice: StateCreator<AppStore, [], [], ServersSlice> = (set, get) => ({
  servers: [],
  serversLoaded: false,

  loadServers: async () => {
    const saved = await commands.loadServers();
    const servers = saved.map((savedServer) => {
      const server = createServer(
        savedServer.name,
        savedServer.address,
        savedServer.category || "",
      );
      server.protocol = savedServer.protocol;
      return server;
    });
    set({ servers, serversLoaded: true });
    for (const server of servers) {
      get().pingServer(server.id, server.ip);
    }
  },

  pingServer: async (id, ip) => {
    const status = await commands.pingServer(ip);
    const servers = get().servers.map((server) => {
      const isTarget = server.id === id;
      if (!isTarget) {
        return server;
      }
      const pinged: Server = {
        ...server,
        online: status.online,
        players: status.players,
        max_players: status.max_players,
        ping: status.ping_ms,
        motd: status.motd,
        version: status.version,
      };
      return pinged;
    });
    set({ servers });
  },

  pingAllServers: () => {
    for (const server of get().servers) {
      get().pingServer(server.id, server.ip);
    }
  },

  addServer: (name, ip, category) => {
    const server = createServer(name, ip, category);
    const servers = [...get().servers, server];
    set({ servers });
    saveServers(servers);
    get().pingServer(server.id, ip);
  },

  editServer: (id, name, ip, category) => {
    const existing = get().servers.find((server) => server.id === id);
    const ipChanged = existing?.ip !== ip;
    const servers = get().servers.map((server) => {
      const isTarget = server.id === id;
      if (!isTarget) {
        return server;
      }
      const protocol = ipChanged ? undefined : server.protocol;
      const edited: Server = { ...server, name, ip, category, protocol };
      return edited;
    });
    set({ servers });
    saveServers(servers);
    if (ipChanged) {
      get().pingServer(id, ip);
    }
  },

  moveServer: (fromId, toId) => {
    const current = get().servers;
    const fromIndex = current.findIndex((server) => server.id === fromId);
    const toIndex = current.findIndex((server) => server.id === toId);
    const fromMissing = fromIndex === -1;
    const toMissing = toIndex === -1;
    const samePlace = fromIndex === toIndex;
    if (fromMissing) {
      return;
    }
    if (toMissing) {
      return;
    }
    if (samePlace) {
      return;
    }
    const servers = [...current];
    const [moved] = servers.splice(fromIndex, 1);
    moved.category = current[toIndex].category;
    servers.splice(toIndex, 0, moved);
    set({ servers });
    saveServers(servers);
  },

  removeServer: (id) => {
    const servers = get().servers.filter((server) => server.id !== id);
    set({ servers });
    saveServers(servers);
  },
});
