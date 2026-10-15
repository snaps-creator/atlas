export type Latency = {
  status: "queued" | "testing" | "ok" | "high_latency" | "timeout" | "unreachable" | "stale" | "not_connected" | "error";
  delay: number | null;
  attempts: number;
  pending?: boolean;
  error?: string | null;
  measuredAt?: number;
  lastSuccessAt?: number;
  lastDelay?: number;
  measurement?: "http_via_node_ms";
  lastAttemptAt?: number;
  attemptId?: number;
  networkGeneration?: number;
  probeId?: string;
  startedAt?: number;
  operationId?: number;
  errorKind?: string;
  scope?: string;
  outboundTag?: string;
  connection?: {status:string;transport:string;observedAt:number;error:string} | null;
};

export const LATENCY_FRESH_MS = 90_000;
export function mergeLatency(old: Latency | undefined, incoming: Latency): Latency {
  // Completion time must not promote a probe which started before a newer one.
  const a = old?.startedAt ?? old?.measuredAt ?? 0;
  const b = incoming.startedAt ?? incoming.measuredAt ?? 0;
  if (old && (a > b || (a === b && (old.operationId ?? 0) > (incoming.operationId ?? 0)))) return old;
  if (incoming.status === "testing" && old && (old.status === "timeout" || old.status === "unreachable" || old.status === "error")) {
    return {...incoming, status:old.status, delay:null, pending:true, error:old.error,
      errorKind:old.errorKind, measuredAt:old.measuredAt, connection:old.connection};
  }
  return freshLatency(incoming)!;
}
export function beginLatency(old: Latency | undefined, startedAt: number): Latency {
  return mergeLatency(old, {status:"testing", delay:null, attempts:0, startedAt, pending:true});
}
export function expireLatencies(values: Record<string,Latency>, now=Date.now()) {
  return Object.fromEntries(Object.entries(values).map(([name,value])=>[name,freshLatency(value,Infinity,now)!]));
}
export function latencyDetails(value?: Latency): string {
  const probe = value?.error ?? (freshLatency(value)?.status === "stale"
    ? "Измерение устарело. Доступность не подтверждена."
    : `HTTP 204 через ${value?.outboundTag ? `outbound ${value.outboundTag}` : "этот прокси"} до Cloudflare; методика 3.0.3, повторный запрос по установленному соединению. UDP, другие сайты и пользовательские соединения не проверены.`);
  const connection=value?.connection;
  return connection ? `${probe} Последняя ошибка ${connection.transport.toUpperCase()}-соединения (${new Date(connection.observedAt).toLocaleTimeString()}): ${connection.error}` : probe;
}

export type LatencyProgress = { batchId: string; probeId: string; name: string; result: Latency };

export function recordLatency(old: Latency | undefined, result: Latency): Latency {
  if (result.status === "testing" || result.status === "queued") {
    return {...mergeLatency(old,result),lastAttemptAt:result.lastAttemptAt ?? Date.now()};
  }
  const now=result.measuredAt ?? Date.now();
  return {...result,measuredAt:now,measurement:"http_via_node_ms",
    lastSuccessAt:result.status === "ok" ? now : old?.lastSuccessAt ?? (old?.status === "ok" ? old.measuredAt : undefined),
    lastDelay:result.delay ?? old?.lastDelay ?? old?.delay ?? undefined};
}

export function applyLatencyProgress(previous: Record<string, Latency>, event: LatencyProgress,
  batchId: string, identities: Record<string,string>, names: Set<string>): Record<string, Latency> {
  if (event.batchId !== batchId || event.probeId !== identities[event.name] ||
      (event.result.probeId !== undefined && event.result.probeId !== event.probeId) || !names.has(event.name)) return previous;
  return {...previous, [event.name]: recordLatency(previous[event.name], mergeLatency(previous[event.name], event.result))};
}

export function retainNodeLatencies(values: Record<string,Latency>, before: Record<string,string>, after: Record<string,string>) {
  return Object.fromEntries(Object.entries(values).filter(([name]) =>
    after[name] !== undefined && before[name] === after[name]));
}

export function failPendingLatencies(previous: Record<string, Latency>, names: string[], error: unknown): Record<string, Latency> {
  const next = {...previous};
  for (const name of names) {
    if (next[name]?.pending && next[name].status !== "testing") {
      next[name] = {...next[name], pending:false};
    } else if (next[name]?.status === "testing" || next[name]?.status === "queued") next[name] = recordLatency(next[name],{
      status: "error", delay: null, attempts: 0, error: String(error), measuredAt: Date.now(),
    });
  }
  return next;
}

export type ProxyHealth = { atlasHealth?: Latency | null; alive?: boolean; history?: {time:string; delay:number}[]; extra?: Record<string, {alive?:boolean;history?:{time:string;delay:number}[]}> };

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
  value = freshLatency(value);
  if (value?.status === "timeout" || value?.status === "unreachable") return "Таймаут";
  if (value?.status === "error") return "Ошибка проверки";
  if (value && ["ok", "high_latency"].includes(value.status) && value.delay !== null && Number.isFinite(value.delay) && value.delay >= 0)
    return `${value.delay} мс`;
  return "";
}

export function freshLatency(value?: Latency, threshold = Infinity, now = Date.now()): Latency | undefined {
  if (!value) return;
  if (["ok","high_latency"].includes(value.status) && (value.measuredAt === undefined || !Number.isFinite(value.measuredAt) ||
      value.delay === null || !Number.isFinite(value.delay) || value.delay < 0)) return {...value,status:"stale",delay:null};
  if (["ok","high_latency"].includes(value.status) && value.measuredAt !== undefined &&
      (now - value.measuredAt >= LATENCY_FRESH_MS || value.measuredAt > now + 1000)) return {...value,status:"stale",delay:null};
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
  const startedAt = Date.now();
  try {
    return await Promise.race([
      request,
      new Promise<Latency>((resolve) => {
        timer = setTimeout(() => resolve({
          status: "error", delay: null, attempts: 0, startedAt, measuredAt:Date.now(),
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
