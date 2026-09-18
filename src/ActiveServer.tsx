import { useEffect, useState } from "react";
import { request } from "./api";
import type { Latency } from "./latency";

type Proxies = Record<string, { now?: string }>;
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

export function ActiveServer({ connected }: { connected: boolean }) {
  const [value, setValue] = useState<{ name: string; delay: number | null } | null>(null);
  useEffect(() => {
    setValue(null);
    if (!connected) return;
    let alive = true, pending = false, measured = "", measuredAt = 0;
    const poll = async () => {
      if (pending) return;
      pending = true;
      try {
        const response = await request<{ proxies: Proxies }>("proxies");
        if (!alive) return;
        const name = resolveServer(response.proxies);
        if (!name) { setValue(null); return; }
        setValue(old => old?.name === name ? old : { name, delay: null });
        if (name !== measured || Date.now() - measuredAt >= 30000) {
          const result = await request<Latency>("latency", { name });
          if (!alive) return;
          setValue({ name, delay: result.status === "ok" ? result.delay : null });
          measured = name;
          measuredAt = Date.now();
        }
      } catch { if (alive) setValue(null); }
      finally { pending = false; }
    };
    void poll();
    const timer = window.setInterval(poll, 5000);
    return () => { alive = false; window.clearInterval(timer); };
  }, [connected]);
  return <div className="active-server" title={value?.name}>
    <strong>{connected ? value?.name ?? "—" : "—"}</strong>
    <small>{connected && value?.delay != null ? `${value.delay} мс` : "—"}</small>
  </div>;
}
