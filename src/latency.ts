export type Latency = {
  status: "testing" | "ok" | "unreachable" | "not_connected" | "error";
  delay: number | null;
  attempts: number;
  error?: string | null;
};

export function latencyLabel(value?: Latency): string {
  if (value?.status === "ok" && value.delay !== null)
    return `${value.delay} мс`;
  if (value?.status === "unreachable") return "Нет ответа";
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
