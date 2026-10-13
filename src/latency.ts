export type Latency = {
  status: "queued" | "testing" | "ok" | "high_latency" | "timeout" | "unreachable" | "stale" | "not_connected" | "error";
  delay: number | null;
  attempts: number;
  error?: string | null;
  measuredAt?: number;
  lastSuccessAt?: number;
  lastDelay?: number;
  measurement?: "http_via_node_ms";
};

export type LatencyProgress = { batchId: string; probeId: string; name: string; result: Latency };

export function recordLatency(old: Latency | undefined, result: Latency): Latency {
  const now=result.measuredAt ?? Date.now();
  return {...result,measuredAt:now,measurement:"http_via_node_ms",
    lastSuccessAt:result.status === "ok" ? now : old?.lastSuccessAt ?? (old?.status === "ok" ? old.measuredAt : undefined),
    lastDelay:result.delay ?? old?.lastDelay ?? old?.delay ?? undefined};
}

export function applyLatencyProgress(previous: Record<string, Latency>, event: LatencyProgress,
  batchId: string, identities: Record<string,string>, names: Set<string>): Record<string, Latency> {
  if (event.batchId !== batchId || event.probeId !== identities[event.name] || !names.has(event.name)) return previous;
  return {...previous, [event.name]: recordLatency(previous[event.name],event.result)};
}

export function retainNodeLatencies(values: Record<string,Latency>, before: Record<string,string>, after: Record<string,string>) {
  return Object.fromEntries(Object.entries(values).filter(([name]) =>
    after[name] !== undefined && before[name] === after[name]));
}

export function failPendingLatencies(previous: Record<string, Latency>, names: string[], error: unknown): Record<string, Latency> {
  const next = {...previous};
  for (const name of names) {
    if (next[name]?.status === "testing" || next[name]?.status === "queued") next[name] = recordLatency(next[name],{
      status: "error", delay: null, attempts: 0, error: String(error), measuredAt: Date.now(),
    });
  }
  return next;
}

export type ProxyHealth = { alive?: boolean; history?: {time:string; delay:number}[]; extra?: Record<string, {alive?:boolean;history?:{time:string;delay:number}[]}> };
export function historyLatency(proxy?: ProxyHealth, testUrl?: string): Latency | undefined {
  // AUTO chooses using the history of its own URL. A successful test against
  // another URL must not conceal a failed AUTO check.
  if (testUrl && proxy?.extra?.[testUrl]) proxy = proxy.extra[testUrl];
  const last = proxy?.history?.at(-1);
  const measuredAt = last ? Date.parse(last.time) : NaN;
  if (!last || !Number.isFinite(measuredAt) || !Number.isFinite(last.delay)) return;
  if (proxy?.alive === false) return {status:"unreachable",delay:null,attempts:1,measuredAt};
  if (proxy?.alive === true && last.delay >= 0) return {status:"ok",delay:last.delay,attempts:1,measuredAt};
}

export async function boundedBatch<T>(request: Promise<T>, timeoutMs = 18000): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    return await Promise.race([request, new Promise<never>((_, reject) => {
      timer = setTimeout(() => reject(new Error("Atlas не завершил групповую проверку за отведённое время")), timeoutMs);
    })]);
  } finally { clearTimeout(timer); }
}

// Results and queued batch work belong to one connection/configuration epoch.
export class LatencyEpoch {
  private key = "";
  private active = new Map<string, symbol>();
  update(key: string): boolean {
    if (key === this.key) return false;
    this.key = key;
    this.active.clear();
    return true;
  }
  begin(key: string, name: string): symbol | undefined {
    if (key !== this.key || this.active.has(name)) return;
    const token = Symbol(name);
    this.active.set(name, token);
    return token;
  }
  current(name: string, token: symbol): boolean { return this.active.get(name) === token; }
  finish(name: string, token: symbol): void {
    if (this.current(name, token)) this.active.delete(name);
  }
}

export function latencyLabel(value?: Latency): string {
  if (value?.status === "ok" && value.delay !== null)
    return `${value.delay} мс`;
  if (value?.status === "high_latency") return `${value.delay} мс · выше порога`;
  if (value?.status === "timeout") return "Таймаут";
  if (value?.status === "unreachable") return "Недоступен";
  if (value?.status === "stale") return "Устарело";
  if (value?.status === "queued") return "В очереди";
  if (value?.status === "testing") return "Проверка…";
  if (value?.status === "error") return "Ошибка проверки";
  if (value?.status === "not_connected") return "Нет подключения";
  return "Не проверен";
}

export function freshLatency(value?: Latency, threshold = Infinity, now = Date.now()): Latency | undefined {
  if (!value) return;
  if (value.status !== "testing" && value.status !== "queued" && value.measuredAt !== undefined &&
      (now - value.measuredAt > 30_000 || value.measuredAt > now + 5000)) return {...value,status:"stale"};
  if (value.status === "ok" && value.delay !== null && value.delay > threshold) return {...value,status:"high_latency"};
  return value;
}
export function latencyRank(value?: Latency): number {
  const fresh = freshLatency(value);
  return fresh?.status === "ok" && fresh.delay !== null ? fresh.delay : Infinity;
}

// Bound the UI even if the native invocation never returns. Late completion
// cannot overwrite a subsequent test because only the raced promise is consumed.
export async function boundedLatency(request: Promise<Latency>, timeoutMs = 90_000): Promise<Latency> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    return await Promise.race([
      request,
      new Promise<Latency>((resolve) => {
        timer = setTimeout(() => resolve({
          status: "error", delay: null, attempts: 0,
          error: "Atlas не завершил проверку вовремя. Доступность сервера не определена.",
        }), timeoutMs);
      }),
    ]);
  } finally {
    clearTimeout(timer);
  }
}

export async function testPool<T>(
  items: T[],
  test: (item: T) => Promise<void>,
  concurrency = 3,
) {
  let next = 0;
  await Promise.all(
    Array.from(
      { length: Math.min(items.length, Math.max(1, concurrency)) },
      async () => {
        while (next < items.length) {
          const item = items[next++];
          await test(item);
        }
      },
    ),
  );
}
