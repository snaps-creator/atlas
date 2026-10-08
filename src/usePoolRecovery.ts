import { allSubscriptions } from "./subscriptionSource";
import { useEffect, useRef, useState } from "react";
import { request } from "./api";
import type { Snapshot } from "./types";

export type PoolHealth = { phase: "idle" | "checking" | "refreshing" | "healthy" | "degraded" | "all_timeout" | "local_error"; text: string; checkedAt: number };
type Proxy = { now?: string; testUrl?: string; alive?: boolean; history?: { delay: number; time?: string }[]; extra?: Record<string, Proxy> };
const PRIMARY = "http://cp.cloudflare.com/generate_204";
const SECONDARY = "https://www.cloudflare.com/cdn-cgi/trace";
export function observedHealth(names: string[], proxies: Record<string, Proxy>, maxAgeMs: number, now = Date.now()): PoolHealth {
  let selected = "ATLAS";
  const seen = new Set<string>();
  while (proxies[selected]?.now && !seen.has(selected)) {
    seen.add(selected); selected = proxies[selected].now!;
  }
  const state = (name: string) => {
    const p = proxies[name];
    const controls = [PRIMARY, SECONDARY].map(url => p?.extra?.[url]).filter(Boolean) as Proxy[];
    const records = (controls.length ? controls : [p]).map(h => {
      const at = Date.parse(h?.history?.at(-1)?.time ?? "");
      return { alive: h?.alive, at, fresh: Number.isFinite(at) && now - at <= maxAgeMs && at <= now + 5000 };
    });
    const fresh = records.filter(r => r.fresh);
    return { alive: fresh.some(r => r.alive === true) ? true : fresh.length === 2 && fresh.every(r => r.alive === false) ? false : undefined,
      at: Math.max(0,...fresh.map(r => r.at)), fresh: fresh.length > 0 };
  };
  const active = state(selected), working = names.filter(n => { const h=state(n); return h.fresh && h.alive === true; }).length;
  if (!active.fresh) return {phase:"idle",text:"Нет свежей проверки выбранного сервера. Доступность не подтверждена.",checkedAt:0};
  if (active.alive !== true) return {phase:"degraded",text:`Выбранный сервер не ответил. В пуле свежих успешных проверок: ${working}.`,checkedAt:active.at};
  return {phase:"healthy",text:`Выбранный сервер ответил на контрольный запрос. В пуле свежих успешных проверок: ${working}.`,checkedAt:active.at};
}
export function exhausted(names: string[], proxies: Record<string, Proxy>): boolean {
  return names.length > 0 && names.every(name => proxies[name]?.alive === false && !!proxies[name]?.history?.length);
}
export function poolVerdict(delays: Record<string, number>, secondaryHealthy: string[] = []): PoolHealth {
  const healthy = Object.values(delays).some(delay => Number.isFinite(delay) && delay >= 0) || secondaryHealthy.length > 0;
  return { phase: healthy ? "healthy" : "all_timeout", checkedAt: Date.now(), text: healthy
    ? "Есть отвечающие VPN-серверы. Проверка передачи через сервер успешна."
    : "Все проверенные серверы не ответили на контрольные запросы. Причина — узлы, целевой сайт или сетевой путь — не установлена." };
}
export function usePoolRecovery(snapshot: Snapshot | null) {
  const latest = useRef(snapshot); latest.current = snapshot;
  const [health, setHealth] = useState<PoolHealth>({ phase: "idle", text: "Доступность пула ещё не проверена.", checkedAt: 0 });
  const manual = useRef<() => void>(() => {});
  useEffect(() => {
    if (!snapshot?.running) { setHealth({ phase: "idle", text: "Atlas не подключён.", checkedAt: 0 }); return; }
    let alive = true, pending = false, nextCheck = 0;
    const publish = (value: PoolHealth) => { if (alive) setHealth(value); };
    const valid = (revision: number | undefined) => alive && latest.current?.running && latest.current.revision === revision;
    const probe = () => request<{ revision: number; delays: Record<string, number>; secondaryHealthy: string[] }>("pool_probe");
    const publishSelected = async (revision: number | undefined) => {
      const { proxies } = await request<{ proxies: Record<string, Proxy> }>("proxies");
      if (!valid(revision)) return;
      const settings = latest.current!.settings;
      publish(observedHealth(allSubscriptions(settings).flatMap(sub => sub.servers.map(server => server.name)), proxies,
        Math.max(30000, (settings.autoTestIntervalSeconds ?? 300) * 2000)));
    };
    const check = async (force = false) => {
      if (!alive || pending || (!force && Date.now() < nextCheck)) return;
      pending = true;
      const initial = latest.current;
      if (!initial?.running) { pending = false; return; }
      let revision = initial.revision;
      try {
        const names = allSubscriptions(initial.settings).flatMap(sub => sub.servers.map(server => server.name));
        if (!names.length) return;
        if (!force) {
          const { proxies } = await request<{ proxies: Record<string, Proxy> }>("proxies");
          if (!valid(revision)) return;
          publish(observedHealth(names, proxies, Math.max(30000, (initial.settings.autoTestIntervalSeconds ?? 300) * 2000)));
          nextCheck = Date.now() + 30000;
          return;
        }
        publish({ phase: "checking", text: "Проверяем серверы всех подписок…", checkedAt: Date.now() });
        const result = await probe();
        if (!valid(revision) || result.revision !== revision) return;
        const verdict = poolVerdict(result.delays, result.secondaryHealthy);
        if (verdict.phase === "healthy") await publishSelected(revision);
        else publish(verdict);
        // Only an explicit diagnostic request scans the full pool. The service
        // owns periodic checks and selection while the UI remains passive.
        nextCheck = Date.now() + 30000;
      } catch (error) {
        if (valid(revision)) publish({ phase: "local_error", text: `Проверка или восстановление Atlas не завершены: ${String(error)}. Недоступность всех VPN-серверов не подтверждена.`, checkedAt: Date.now() });
        nextCheck = Date.now() + 5000;
      } finally { pending = false; }
    };
    manual.current = () => { void check(true); };
    // Opening the UI reads the service's observations without a full-node scan.
    void check();
    const timer = window.setInterval(() => { void check(); }, 5000);
    return () => { alive = false; clearInterval(timer); manual.current = () => {}; };
  }, [snapshot?.running]);
  return { health, checkPool: () => manual.current() };
}
