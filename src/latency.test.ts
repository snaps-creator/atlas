import { expect, test, vi } from "vitest";
import { matchingLatencies } from "./latencyPolling";
import { freshLatency, latencyRank, mergeLatency, expireLatencies } from "./latency";

test("свежий пинг отображается числом; timeout остаётся видимым без старого ping", () => {
  const now=Date.now();
  const measured: Latency = {status:"ok",delay:210,attempts:1,measuredAt:now-1000};
  expect(latencyLabel(measured)).toBe("210 мс");
  expect(latencyLabel(freshLatency(measured,150,now))).toBe("210 мс");
  expect(latencyLabel(freshLatency(measured,150,now+10000))).toBe("");
  for (const status of ["ok", "high_latency", "stale"] as const) {
    expect(latencyLabel({status,delay:null,attempts:0})).toBe("");
  }
  expect(latencyLabel({status:"timeout",delay:null,lastDelay:210,attempts:1})).toBe("Таймаут");
});

test("устаревшее измерение и превышение порога не выглядят свежим успехом", () => {
  const value: Latency = {status:"ok",delay:80,attempts:1,measuredAt:1000};
  expect(freshLatency(value,150,2000)?.status).toBe("ok");
  expect(freshLatency({...value,delay:200},150,2000)?.status).toBe("high_latency");
  expect(freshLatency(value,150,32000)?.status).toBe("stale");
  expect(latencyRank(value)).toBe(Infinity);
  expect(latencyLabel({status:"timeout",delay:null,attempts:1})).toBe("Таймаут");
  expect(latencyLabel({status:"queued",delay:null,attempts:0})).toBe("");
});

test("сбой хранит последнее успешное измерение и реальное время попытки", () => {
  const names=new Set(["node"]), identities={node:"identity"};
  const now=Date.now();
  const ok=applyLatencyProgress({}, {batchId:"b",probeId:"identity",name:"node",result:{status:"ok",delay:60,attempts:1,measuredAt:now-1000}},"b",identities,names);
  const failure=applyLatencyProgress(ok,{batchId:"b",probeId:"identity",name:"node",result:{status:"timeout",delay:null,attempts:1,measuredAt:now}},"b",identities,names);
  expect(failure.node).toMatchObject({lastSuccessAt:now-1000,lastDelay:60,measuredAt:now,status:"timeout"});
});
import { retainNodeLatencies, applyLatencyProgress, failPendingLatencies, boundedLatency, boundedBatch, latencyLabel, testPool, LatencyEpoch, type Latency } from "./latency";

test("смена выбора сохраняет результаты; замена одной подписки не стирает остальные", () => {
  const values: Record<string,Latency> = {Berlin:{status:"ok",delay:42,attempts:1,measuredAt:Date.now()}, Paris:{status:"testing",delay:null,attempts:0}};
  const before = {Berlin:"b1",Paris:"p1"};
  expect(retainNodeLatencies(values,before,{...before})).toEqual(values);
  const after = {Berlin:"b2",Paris:"p1"};
  const kept = retainNodeLatencies(values,before,after);
  expect(kept).toEqual({Paris:values.Paris});
  const event = {batchId:"batch",probeId:"p1",name:"Paris",result:{status:"ok" as const,delay:50,attempts:1,measuredAt:Date.now()}};
  expect(applyLatencyProgress(kept,event,"batch",after,new Set(["Berlin","Paris"])).Paris.delay).toBe(50);
  expect(applyLatencyProgress(kept,{...event,name:"Berlin",probeId:"b1"},"batch",after,new Set(["Berlin","Paris"]))).toBe(kept);
});

test("готовый сервер появляется до завершения остальных; поздние чужие результаты игнорируются", () => {
  const pending: Latency = {status:"testing",delay:null,attempts:0};
  const initial = {fast: pending, slow: pending};
  const names = new Set(["fast", "slow"]);
  const event = {batchId:"current",probeId:"node-v1",name:"fast",result:{status:"ok" as const,delay:42,attempts:1,measuredAt:Date.now()}};
  const next = applyLatencyProgress(initial,event,"current",{fast:"node-v1",slow:"slow-v1"},names);
  expect(next.fast.delay).toBe(42);
  expect(next.slow.status).toBe("testing");
  expect(applyLatencyProgress(next,{...event,batchId:"old"},"current",{fast:"node-v1",slow:"slow-v1"},names)).toBe(next);
  expect(applyLatencyProgress(next,{...event,probeId:"node-v0"},"current",{fast:"node-v1",slow:"slow-v1"},names)).toBe(next);
  expect(applyLatencyProgress(next,{...event,name:"foreign"},"current",{fast:"node-v1",slow:"slow-v1"},names)).toBe(next);
  const failed = failPendingLatencies(next,["fast","slow"],"deadline");
  expect(failed.fast).toEqual(next.fast);
  expect(failed.slow.status).toBe("error");
});


test("групповая проверка не зависает при потере IPC и не принимает поздний ответ", async () => {
  vi.useFakeTimers();
  try {
    let finish!: (v: number) => void;
    const request = new Promise<number>(resolve => { finish = resolve; });
    const result = boundedBatch(request).catch(error => String(error));
    await vi.advanceTimersByTimeAsync(18000);
    expect(await result).toContain("отведённое время");
    finish(42);
    expect(await result).toContain("отведённое время");
    expect(vi.getTimerCount()).toBe(0);
  } finally { vi.useRealTimers(); }
});

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
  expect(latencyLabel()).toBe("");
  expect(latencyLabel({ status: "error", delay: null, attempts: 0 })).toBe("Ошибка проверки");
  expect(
    latencyLabel({ status: "unreachable", delay: null, attempts: 4 }),
  ).toBe("Таймаут");
  expect(latencyLabel({ status: "ok", delay: 0, attempts: 1, measuredAt:Date.now() })).toBe("0 мс");
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

test("быстрый результат публикуется, пока медленный сервер ещё проверяется", async () => {
  let release!: () => void;
  const slow = new Promise<void>(resolve => { release = resolve; });
  const shown: string[] = [];
  let finished = false;
  const batch = testPool(["slow", "fast"], async name => {
    if (name === "slow") await slow;
    shown.push(name);
  }, 2).then(() => { finished = true; });
  await Promise.resolve();
  expect(shown).toEqual(["fast"]);
  expect(finished).toBe(false);
  release();
  await batch;
  expect(shown).toEqual(["fast", "slow"]);
});


test("успешные 56 мс исчезают при timeout и истечении 10 секунд", () => {
  const ok: Latency = {status:"ok",delay:56,attempts:1,startedAt:100,measuredAt:200};
  expect(freshLatency(ok,Infinity,10_199)?.status).toBe("ok");
  expect(freshLatency(ok,Infinity,10_200)).toMatchObject({status:"stale",delay:null});
  const failed: Latency = {status:"unreachable",delay:null,attempts:1,startedAt:300,measuredAt:400};
  expect(mergeLatency(ok,failed)).toEqual(failed);
  expect(mergeLatency(failed,{...ok,measuredAt:500})).toEqual(failed);
  expect(expireLatencies({a:ok,b:failed},10_200).a.delay).toBeNull();
  expect(expireLatencies({a:ok,b:failed},10_200).b).toEqual(failed);
});

test("поздний итог batch не освежает ранний результат и не заменяет новую ошибку", () => {
  const now=Date.now();
  const failed: Latency={status:"unreachable",delay:null,attempts:1,startedAt:now-100,measuredAt:now};
  const late: Latency={status:"ok",delay:56,attempts:1,startedAt:now-1000,measuredAt:now};
  expect(mergeLatency(failed,late)).toEqual(failed);
  expect(mergeLatency(undefined,{...late,measuredAt:now-11_000})).toMatchObject({status:"stale",delay:null});
  expect(freshLatency({status:"ok",delay:56,attempts:1})).toMatchObject({status:"stale",delay:null});
});


test("UI consumes backend pending failure without creating a separate probe schedule", () => {
  const failed:Latency={status:"unreachable",delay:null,attempts:1,errorKind:"timeout",measuredAt:100};
  const snapshot={results:{node:failed},identities:{node:"current"},pending:["node"],generation:2};
  expect(matchingLatencies(snapshot,{node:"current"}).node).toMatchObject({status:"unreachable",delay:null,pending:true,measuredAt:100});
  expect(matchingLatencies(snapshot,{node:"replaced"})).toEqual({});
});
