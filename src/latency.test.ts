import { expect, test, vi } from "vitest";
import { boundedLatency, latencyLabel, testPool, LatencyEpoch, type Latency } from "./latency";

test("ответ старой сессии и её очередь не затрагивают новую проверку", () => {
  const epoch = new LatencyEpoch();
  epoch.update("session-1");
  const old = epoch.begin("session-1", "Berlin")!;
  expect(epoch.begin("session-1", "Berlin")).toBeUndefined();
  epoch.update("session-2");
  expect(epoch.current("Berlin", old)).toBe(false);
  expect(epoch.begin("session-1", "Paris")).toBeUndefined();
  const current = epoch.begin("session-2", "Berlin")!;
  epoch.finish("Berlin", old);
  expect(epoch.current("Berlin", current)).toBe(true);
  epoch.finish("Berlin", current);
  expect(epoch.current("Berlin", current)).toBe(false);
});

test("ошибка контроллера не означает недоступность сервера", () => {
  expect(latencyLabel()).toBe("—");
  expect(latencyLabel({ status: "error", delay: null, attempts: 0 })).toBe("Ошибка проверки");
  expect(
    latencyLabel({ status: "unreachable", delay: null, attempts: 4 }),
  ).toBe("Таймаут");
  expect(latencyLabel({ status: "ok", delay: 0, attempts: 1 })).toBe("0 мс");
});

test("зависший native-запрос освобождает очередь, поздний ответ не меняет результат", async () => {
  vi.useFakeTimers();
  try {
    let finish!: (value: Latency) => void;
    const hanging = new Promise<Latency>((resolve) => { finish = resolve; });
    const results: Latency[] = [];
    const work = testPool([hanging, Promise.resolve<Latency>({ status: "ok", delay: 20, attempts: 1 })], async (p) => {
      results.push(await boundedLatency(p, 100));
    }, 1);
    await vi.advanceTimersByTimeAsync(101);
    await work;
    expect(results.map(r => r.status)).toEqual(["error", "ok"]);
    finish({ status: "ok", delay: 100, attempts: 1 });
    await Promise.resolve();
    expect(results[0].status).toBe("error");
    expect(vi.getTimerCount()).toBe(0);
  } finally { vi.useRealTimers(); }
});

test("проверки ограничены и результаты поступают до завершения очереди", async () => {
  let active = 0,
    maximum = 0;
  const done: number[] = [];
  await testPool([1, 2, 3, 4, 5, 6, 7], async (n) => {
    maximum = Math.max(maximum, ++active);
    await new Promise((resolve) => setTimeout(resolve, n === 1 ? 20 : 1));
    done.push(n);
    --active;
  });
  expect(maximum).toBe(3);
  expect(done[0]).not.toBe(1);
  expect(done.sort()).toEqual([1, 2, 3, 4, 5, 6, 7]);
});
