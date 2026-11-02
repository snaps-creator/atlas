import type { Connection } from "./types";

export const sites = [
  ["chatgpt", "ChatGPT", "chatgpt.com"],
  ["grok", "Grok", "grok.com"],
  ["gemini", "Gemini", "gemini.google.com"],
  ["youtube", "YouTube", "www.youtube.com"],
  ["telegram", "Telegram", "web.telegram.org"],
] as const;

export type SiteResult = {
  ok: boolean;
  ms: number | null;
  status: number | null;
  error?: string;
  errorKind?: "local" | "timeout" | "network";
  outcome?: "http_response" | "regional_hint" | "auth_required" | "http_error" | "timeout" | "network_error" | "cancelled" | "rtt" | "unsupported";
  detail?: string;
  revision?: number;
  bodyLimited?: boolean;
  metric?: "h2_ping_rtt";
};

export function siteResultLabel(result: SiteResult): string {
  const speed = siteSpeed(result);
  if (speed) return `${result.ms} мс · ${speed === "fast" ? "Быстро" : speed === "medium" ? "Средне" : "Медленно"}`;
  if (result.metric === "h2_ping_rtt" && result.outcome !== "cancelled") return "Пинг не измерен";
  if (result.outcome === "unsupported") return "Пинг недоступен";
  if (result.outcome === "regional_hint") return "Текст об ограничении региона";
  if (result.outcome === "auth_required") return "Требуется авторизация";
  if (result.outcome === "cancelled") return "Проверка отменена";
  if (result.outcome === "timeout") return "Таймаут запроса";
  if (result.outcome === "network_error") return "Ошибка соединения";
  if (result.status != null) return result.ok
    ? `HTTP-ответ: ${result.ms} мс` : `HTTP ${result.status}`;
  if (result.errorKind === "timeout") return "Таймаут запроса";
  if (result.errorKind === "network") return "Ошибка соединения";
  return "Проверка не выполнена";
}

// These thresholds describe RTT, not download bandwidth or service availability.
export function siteSpeed(result?: SiteResult): "fast" | "medium" | "slow" | undefined {
  if (!result?.ok || result.metric !== "h2_ping_rtt" || result.outcome !== "rtt"
    || result.ms == null || !Number.isFinite(result.ms) || result.ms < 0) return;
  return result.ms < 150 ? "fast" : result.ms < 400 ? "medium" : "slow";
}

export function siteResultClass(result?: SiteResult): string {
  const speed = siteSpeed(result);
  if (speed) return `site-speed-${speed}`;
  return "";
}

export function siteResultWarning(result?: SiteResult): boolean {
  return !!result && result.outcome !== "cancelled" && (result.outcome
    ? result.outcome !== "http_response" : !result.ok);
}

// Evidence from existing connection polling. Never substitute selected server,
// never attribute a domain match to this HTTP probe or to a logged-in account.
export function observedSiteConnections(host: string, connections: Connection[], observedAt: number, now = Date.now()): Connection[] {
  if (!observedAt || now - observedAt > 10_000 || now < observedAt) return [];
  return connections.filter(c => {
    if (typeof c.metadata?.host !== "string") return false;
    const domain = c.metadata.host.toLowerCase().replace(/\.$/, "");
    return domain === host || domain.endsWith(`.${host}`);
  });
}

type Request = <T>(action: string, payload?: unknown) => Promise<T>;
export function startSiteChecks(invoke: Request, revision: number, epoch: number, receive: (id: string, result: SiteResult) => void) {
  let cancelled = false;
  let generation: number | undefined;
  const cancel = () => {
    cancelled = true;
    if (generation !== undefined) {
      const token = generation;
      generation = undefined;
      void invoke("site_checks_cancel", { generation: token }).catch(() => {});
    }
  };
  const done = (async () => {
    try {
      generation = await invoke<number>("site_checks_begin");
      if (cancelled) { cancel(); return; }
      let next = 0;
      await Promise.all(Array.from({ length: 2 }, async () => {
        while (!cancelled && next < sites.length) {
          const [id] = sites[next++];
          let result: SiteResult;
          try {
            result = await invoke<SiteResult>("site_check", { id, generation, revision, epoch });
          } catch (e) {
            result = { ok: false, ms: null, status: null, errorKind: "local", error: String(e) };
          }
          if (!cancelled && (result.revision === undefined || result.revision === revision)) receive(id, result);
        }
      }));
    } catch (e) {
      if (!cancelled) for (const [id] of sites) receive(id, { ok:false, ms:null, status:null, errorKind:"local", error:String(e) });
    } finally { cancel(); }
  })();
  return { cancel, done };
}
