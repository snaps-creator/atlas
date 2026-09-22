export type Latency = {
  status: "testing" | "ok" | "unreachable" | "not_connected" | "error";
  delay: number | null;
  attempts: number;
  error?: string | null;
  measuredAt?: number;
};

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
      timer = setTimeout(() => reject(new Error("Atlas не завершил групповую проверку за 18 секунд")), timeoutMs);
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
  if (value?.status === "unreachable") return "Таймаут";
  if (value?.status === "testing") return "Проверка…";
  if (value?.status === "error") return "Ошибка проверки";
  if (value?.status === "not_connected") return "Нет подключения";
  return "—";
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
