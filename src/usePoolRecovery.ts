import { useEffect, useRef, useState } from "react";
import { request } from "./api";
import type { Snapshot } from "./types";

export type PoolHealth = { phase: "idle" | "checking" | "refreshing" | "healthy" | "all_timeout" | "local_error"; text: string; checkedAt: number };
type Proxy = { now?: string; alive?: boolean; history?: { delay: number }[] };
export function exhausted(names: string[], proxies: Record<string, Proxy>): boolean {
  return names.length > 0 && names.every(name => proxies[name]?.alive === false && !!proxies[name]?.history?.length);
}
export function poolVerdict(delays: Record<string, number>, refreshFailed: boolean): PoolHealth {
  const healthy = Object.values(delays).some(delay => Number.isFinite(delay) && delay >= 0);
  return { phase: healthy ? "healthy" : "all_timeout", checkedAt: Date.now(), text: healthy
    ? "Есть отвечающие VPN-серверы. Проверка передачи через сервер успешна."
    : refreshFailed ? "Все серверы — таймаут. Подписки обновить удалось не полностью. Причина недоступности не установлена."
    : "Все серверы — таймаут после обновления подписок. Нет ответа через VPN; сбой серверов или сетевого пути — причина не установлена." };
}
export function usePoolRecovery(snapshot: Snapshot | null, update: (value: Snapshot) => void) {
  const latest = useRef(snapshot); latest.current = snapshot;
  const [health, setHealth] = useState<PoolHealth>({ phase: "idle", text: "Доступность пула ещё не проверена.", checkedAt: 0 });
  const manual = useRef<() => void>(() => {});
  useEffect(() => {
    if (!snapshot?.running) { setHealth({ phase: "idle", text: "Atlas не подключён.", checkedAt: 0 }); return; }
    let alive = true, pending = false, recovered = false, nextCheck = 0, verifyAgain = false;
    const publish = (value: PoolHealth) => { if (alive) setHealth(value); };
    const valid = (revision: number | undefined) => alive && latest.current?.running && latest.current.revision === revision;
    const probe = () => request<{ revision: number; delays: Record<string, number> }>("pool_probe");
    const check = async (force = false) => {
      if (!alive || pending || (!force && Date.now() < nextCheck)) return;
      pending = true;
      const initial = latest.current;
      if (!initial?.running) { pending = false; return; }
      let revision = initial.revision;
      try {
        const names = initial.settings.subscriptions.flatMap(sub => sub.servers.map(server => server.name));
        if (!names.length) return;
        if (!force && !verifyAgain) {
          const { proxies } = await request<{ proxies: Record<string, Proxy> }>("proxies");
          if (!valid(revision) || !exhausted(names, proxies)) return;
        }
        publish({ phase: "checking", text: "Проверяем серверы всех подписок…", checkedAt: Date.now() });
        let result = await probe();
        if (!valid(revision) || result.revision !== revision) return;
        let verdict = poolVerdict(result.delays, false);
        if (verdict.phase === "healthy") { recovered = false; verifyAgain = false; publish(verdict); nextCheck = Date.now() + 30000; return; }
        verifyAgain = true;
        if (!recovered) {
          recovered = true;
          publish({ phase: "refreshing", text: "Все серверы — таймаут. Обновляем подписки…", checkedAt: Date.now() });
          const refreshed = await request<{ snapshot: Snapshot; failedSubscriptions: string[] }>("pool_refresh", { revision });
          if (!alive) return;
          revision = refreshed.snapshot.revision;
          latest.current = refreshed.snapshot; update(refreshed.snapshot);
          publish({ phase: "checking", text: "Подписки обработаны. Повторно проверяем общий пул…", checkedAt: Date.now() });
          result = await probe();
          if (!valid(revision) || result.revision !== revision) return;
          verdict = poolVerdict(result.delays, refreshed.failedSubscriptions.length > 0);
        }
        publish(verdict);
        if (verdict.phase === "healthy") { recovered = false; verifyAgain = false; }
        nextCheck = Date.now() + 60000;
      } catch (error) {
        verifyAgain = true;
        if (valid(revision)) publish({ phase: "local_error", text: `Проверка или восстановление Atlas не завершены: ${String(error)}. Недоступность всех VPN-серверов не подтверждена.`, checkedAt: Date.now() });
        nextCheck = Date.now() + 60000;
      } finally { pending = false; }
    };
    manual.current = () => { void check(true); };
    void check(true);
    const timer = window.setInterval(() => { void check(); }, 5000);
    return () => { alive = false; clearInterval(timer); manual.current = () => {}; };
  }, [snapshot?.running, update]);
  return { health, checkPool: () => manual.current() };
}
