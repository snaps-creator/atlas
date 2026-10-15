import { serverDisplayName } from "./serverDisplayName";
import { freshLatency, latencyDetails, type Latency, type ProxyHealth } from "./latency";
import type { PoolHealth } from "./usePoolRecovery";

type Proxies = Record<string, ProxyHealth & { now?: string; testUrl?:string }>;
export function selectedLatency(proxies: Proxies): Latency | undefined {
  const server = resolveServer(proxies);
  if (!server) return;
  return freshLatency(proxies[server]?.atlasHealth ?? undefined);
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

export function ActiveServer({ connected, name, latency, poolHealth }: {
  connected: boolean; name: string | null; latency?: Latency; poolHealth?: PoolHealth;
}) {
  const value = freshLatency(latency);
  return <div className="active-server" title={name ? serverDisplayName(name) : undefined}>
    <strong>{connected && name ? serverDisplayName(name) : "—"}</strong>
    <small className={connected && ((value?.status === "unreachable" || value?.status === "timeout") || value?.status === "error") ? "pool-error" : ""}
      title={value ? latencyDetails(value) : poolHealth?.text}>
      {!connected ? "—" : (value?.status === "unreachable" || value?.status === "timeout") ? "Таймаут выбранного сервера" : value?.status === "error" ? "Ошибка проверки" : value?.status === "ok" ? `${value.delay} мс` : value?.status === "testing" ? "Проверка…" : "—"}
    </small>
  </div>;
}
