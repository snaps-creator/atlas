export type Latency = {
  status: "testing" | "ok" | "unreachable" | "not_connected" | "error";
  delay: number | null;
  attempts: number;
  error?: string | null;
};

export function latencyLabel(value?: Latency): string {
  if (value?.status === "ok" && value.delay !== null)
    return `${value.delay} мс`;
  if (value?.status === "unreachable") return "Недоступен";
  if (value?.status === "testing") return "Проверка…";
  return "—";
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
