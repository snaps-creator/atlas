import { expect, test } from "vitest";
import { latencyLabel, testPool } from "./latency";

test("ошибка контроллера не означает недоступность сервера", () => {
  expect(latencyLabel()).toBe("—");
  expect(latencyLabel({ status: "error", delay: null, attempts: 0 })).toBe("—");
  expect(
    latencyLabel({ status: "unreachable", delay: null, attempts: 4 }),
  ).toBe("Недоступен");
  expect(latencyLabel({ status: "ok", delay: 0, attempts: 1 })).toBe("0 мс");
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
