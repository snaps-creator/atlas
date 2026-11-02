import { expect, test } from "vitest";
import { siteResultLabel, siteResultWarning, siteResultClass, siteSpeed, observedSiteConnections, startSiteChecks, type SiteResult } from "./siteChecks";
import type { Connection } from "./types";

test("local contention, network failure and HTTP refusal remain distinct", () => {
  const failed = {ok:false,ms:null,status:null};
  expect(siteResultLabel({...failed,errorKind:"local"})).toBe("Проверка не выполнена");
  expect(siteResultLabel({...failed,errorKind:"timeout"})).toBe("Таймаут запроса");
  expect(siteResultLabel({...failed,errorKind:"network"})).toBe("Ошибка соединения");
  expect(siteResultLabel({...failed,status:403})).toBe("HTTP 403");
  expect(siteResultLabel({ok:true,status:200,ms:42})).toBe("HTTP-ответ: 42 мс");
});

const http: SiteResult = {ok:true,ms:42,status:200,outcome:"http_response",revision:7};
test("RTT colors have explicit boundaries and never turn HTTP or incomplete results green", () => {
  const rtt: SiteResult = {ok:true,ms:42,status:null,metric:"h2_ping_rtt",outcome:"rtt"};
  for (const [ms,speed] of [[149,"fast"],[150,"medium"],[399,"medium"],[400,"slow"]] as const) {
    expect(siteSpeed({...rtt,ms})).toBe(speed);
    expect(siteResultClass({...rtt,ms})).toBe(`site-speed-${speed}`);
  }
  expect(siteResultLabel(rtt)).toBe("42 мс · Быстро");
  for (const result of [http,{...rtt,ms:null},{...rtt,ms:NaN},{...rtt,ms:-1},{...rtt,ok:false}]) {
    expect(siteSpeed(result)).toBeUndefined();
    expect(siteResultClass(result)).toBe("");
  }
  for (const outcome of ["unsupported", "timeout", "network_error"] as const) {
    const missing = {...rtt,ok:false,ms:null,outcome};
    expect(siteResultLabel(missing)).toBe("Пинг не измерен");
    expect(siteResultClass(missing)).toBe("");
  }
});
const deferred = <T,>() => {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(r => { resolve = r; });
  return { promise, resolve };
};
const tick = async () => { await Promise.resolve(); await Promise.resolve(); };

test("HTTP 200 with a regional hint is not presented as successful service health", () => {
  expect(siteResultWarning(http)).toBe(false);
  expect(siteResultWarning({...http,outcome:"regional_hint"})).toBe(true);
  expect(siteResultLabel({...http,outcome:"regional_hint"})).toContain("ограничении региона");
  expect(siteResultLabel({...http,outcome:"auth_required"})).toBe("Требуется авторизация");
  expect(siteResultLabel({...http,outcome:"cancelled"})).toBe("Проверка отменена");
});

test("existing connections keep their actual chain; host boundaries and snapshot age matter", () => {
  const c = {id:"old-flow",metadata:{host:"gemini.google.com"},chains:["New York", "ATLAS"]} as Connection;
  const unrelated = {...c,id:"other",metadata:{...c.metadata,host:"notgemini.google.com"}};
  expect(observedSiteConnections("gemini.google.com",[c,unrelated],1000,2000)).toEqual([c]);
  expect(observedSiteConnections("gemini.google.com",[c],1000,11001)).toEqual([]);
  expect(observedSiteConnections("gemini.google.com",[c],0,2000)).toEqual([]);
  expect(observedSiteConnections("gemini.google.com",[{} as Connection,{metadata:{host:null}} as unknown as Connection],1000,2000)).toEqual([]);
});

test("two workers bound the batch and cancellation stops queued work and late results", async () => {
  const pending: ReturnType<typeof deferred<SiteResult>>[] = [];
  const received: SiteResult[] = [];
  const cancelled: unknown[] = [];
  const invoke = async <T,>(action: string, payload?: unknown): Promise<T> => {
    if (action === "site_checks_begin") return 12 as T;
    if (action === "site_checks_cancel") { cancelled.push(payload); return null as T; }
    expect(payload).toMatchObject({generation:12,revision:7,epoch:3});
    const item = deferred<SiteResult>(); pending.push(item); return item.promise as Promise<T>;
  };
  const batch = startSiteChecks(invoke,7,3,(_id,result)=>received.push(result));
  await tick(); expect(pending).toHaveLength(2);
  pending[0].resolve(http); await tick(); expect(pending).toHaveLength(3);
  batch.cancel(); pending[1].resolve(http); pending[2].resolve(http);
  await batch.done;
  expect(received).toHaveLength(1); expect(pending).toHaveLength(3);
  expect(cancelled).toEqual([{generation:12}]);
});

test("unmount while begin is pending cancels that batch without issuing any site requests", async () => {
  const begin = deferred<number>(); const actions: string[] = [];
  const invoke = async <T,>(action: string): Promise<T> => {
    actions.push(action); return (action === "site_checks_begin" ? begin.promise : Promise.resolve(null)) as Promise<T>;
  };
  const batch = startSiteChecks(invoke,7,3,()=>{ throw new Error("late result"); });
  batch.cancel(); begin.resolve(12); await batch.done;
  expect(actions).toEqual(["site_checks_begin","site_checks_cancel"]);
});

test("a result from another configuration cannot update the current UI", async () => {
  const received: SiteResult[] = [];
  const invoke = async <T,>(action: string): Promise<T> =>
    (action === "site_checks_begin" ? 12 : {...http,revision:8}) as T;
  await startSiteChecks(invoke,7,3,(_id,r)=>received.push(r)).done;
  expect(received).toEqual([]);
});
