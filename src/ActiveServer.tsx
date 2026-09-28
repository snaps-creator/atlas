import { useEffect, useState } from "react";
import { request } from "./api";
import { historyLatency, type Latency, type ProxyHealth } from "./latency";
import type { PoolHealth } from "./usePoolRecovery";

type Proxies = Record<string, ProxyHealth & { now?: string; testUrl?:string }>;
export function selectedLatency(proxies: Proxies): Latency | undefined {
  const server = resolveServer(proxies);
  if (!server) return;
  let name = "ATLAS", testUrl: string | undefined;
  while (proxies[name]?.now) {
    testUrl = proxies[name].testUrl ?? testUrl;
    name = proxies[name].now!;
  }
  return historyLatency(proxies[server], testUrl);
}
export function resolveServer(proxies: Proxies): string | null {
  let name = "ATLAS";
  const visited = new Set<string>();
  while (proxies[name]?.now) {
    if (visited.has(name)) return null;
    visited.add(name);
    name = proxies[name].now!;
  }
  return visited.size && proxies[name] ? name : null;
}

export function ActiveServer({
  connected,
  onChange,
  poolHealth,
}: {
  connected: boolean;
  onChange?: (name: string | null) => void;
  poolHealth?: PoolHealth;
}) {
  const [value, setValue] = useState<{ name: string; delay: number | null; status?:Latency["status"] } | null>(null);
  const [controlError, setControlError] = useState<string | null>(null);
  useEffect(() => {
    setValue(null);
    setControlError(null);
    onChange?.(null);
    if (!connected) return;
    let alive = true, pending = false;
    const poll = async () => {
      if (pending) return;
      pending = true;
      try {
        const response = await request<{ proxies: Proxies }>("proxies");
        if (!alive) return;
        setControlError(null);
        const name = resolveServer(response.proxies);
        if (!name) { setValue(null); onChange?.(null); return; }
        onChange?.(name);
        const result = selectedLatency(response.proxies);
        setValue({name,delay:result?.delay ?? null,status:result?.status});
      } catch (error) { if (alive) setControlError(String(error)); }
      finally { pending = false; }
    };
    void poll();
    const timer = window.setInterval(poll, 5000);
    return () => { alive = false; window.clearInterval(timer); };
  }, [connected, onChange]);
  return <div className="active-server" title={value?.name}>
    <strong>{connected ? value?.name ?? "—" : "—"}</strong>
    <small className={connected && (controlError || value?.status === "unreachable" || poolHealth?.phase === "all_timeout" || poolHealth?.phase === "refreshing") ? "pool-error" : ""} title={controlError ?? poolHealth?.text}>{connected && controlError ? "Не удалось обновить статус" : connected && (poolHealth?.phase === "all_timeout" || poolHealth?.phase === "refreshing") ? "Все проверки — таймаут" : connected && value?.status === "unreachable" ? "Таймаут выбранного сервера" : connected && value?.delay != null ? `${value.delay} мс` : "—"}</small>
  </div>;
}
