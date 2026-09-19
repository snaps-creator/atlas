export type Route = "PROXY" | "DIRECT" | "BLOCK";
export type Rule = { kind: string; value: string; noResolve: boolean };
export type Group = {
  id: string;
  name: string;
  description: string;
  enabled: boolean;
  route: Route;
  rules: Rule[];
};
export type Server = { name: string; type: string; country?: string | null };
export type Subscription = {
  id: string;
  name: string;
  maskedUrl: string;
  updatedAt: number;
  error: string | null;
  servers: Server[];
};
export type Settings = {
  groups: Group[];
  subscriptions: Subscription[];
  selected: string;
  defaultRoute: Route;
  mode: string;
  dns: { servers: string[]; ipv6: boolean; fakeIp: boolean };
  startup: {
    launchWithWindows: boolean;
    autoConnect: boolean;
    startInTray: boolean;
    delaySeconds: number;
    restoreConnection: boolean;
  };
  autoTestIntervalSeconds: number;
  theme: string;
  wasConnected: boolean;
  favorites: string[];
};
export type Log = { time: number; level: string; message: string };
export type Snapshot = {
  settings: Settings;
  status: string;
  running: boolean;
  guardActive: boolean;
  error: string | null;
  duration: number;
  logs: Log[];
};
export type Connection = {
  id: string;
  metadata: {
    process: string;
    processPath: string;
    host: string;
    destinationIP: string;
    destinationPort: string;
    network: string;
    type: string;
  };
  upload: number;
  download: number;
  rule: string;
  rulePayload: string;
  chains: string[];
  start: string;
};
