import { expect, test } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { ServerCard } from "./ServerCard";
import { ActiveServer } from "./ActiveServer";
import { beginLatency, mergeLatency, expireLatencies, retainNodeLatencies, latencyDetails, LATENCY_FRESH_MS, type Latency } from "./latency";
import { selectedLatency } from "./ActiveServer";

const render = (latency: Latency) => ({
  card:renderToStaticMarkup(<ServerCard server={{name:"fixture",type:"socks5",probeId:"revision"}}
    selected favorite={false} disabled={false} testing={latency.status === "testing"} latency={latency}
    onSelect={()=>{}} onFavorite={()=>{}} onTest={()=>{}} />),
  active:renderToStaticMarkup(<ActiveServer connected name="fixture" latency={latency} />),
});
test("карточка и активный сервер согласованы: успех, обрыв, устаревание, восстановление", () => {
  const ok:Latency={status:"ok",delay:56,attempts:1,measuredAt:Date.now()};
  const good=render(ok);
  expect(good.card).toContain("server-card-delay fast");expect(good.active).toContain("56 мс");
  for (const state of ["unreachable","error","testing","stale"] as const) {
    // Even a malformed failed record containing an old numeric delay is safe.
    const result=render({...ok,status:state});
    expect(result.card).not.toContain("server-card-delay fast");expect(result.active).not.toContain("56 мс");
  }
  const stale=render({...ok,measuredAt:Date.now()-LATENCY_FRESH_MS-1});
  expect(stale.card).not.toContain("server-card-delay fast");expect(stale.active).not.toContain("56 мс");
  expect(render({...ok,measuredAt:Date.now()}).active).toContain("56 мс");
});
test("ошибка UDP и пользовательского соединения не подменяет результат TCP-теста", () => {
  const value:Latency={status:"ok",delay:56,attempts:1,measuredAt:Date.now(),
    connection:{status:"error",transport:"udp",observedAt:Date.now(),error:"STUN не ответил"}};
  expect(render(value).card).toContain("server-card-delay fast");
  expect(latencyDetails(value)).toContain("STUN не ответил");
  expect(latencyDetails(value)).toContain("пользовательские соединения не проверены");
});

test("timeout остаётся в реальных компонентах при повторной проверке, рендере и открытии Servers", () => {
  const now=Date.now();
  const ok:Latency={status:"ok",delay:56,attempts:1,startedAt:now-1000,measuredAt:now-900,operationId:1};
  const timeout:Latency={status:"unreachable",delay:null,attempts:1,startedAt:now-500,measuredAt:now-100,
    operationId:2,errorKind:"timeout",error:"Mihomo API: HTTP 504"};
  let value=mergeLatency(ok,timeout);
  value=beginLatency(value,now);
  value=expireLatencies({fixture:value},now+20_000).fixture;
  value=retainNodeLatencies({fixture:value},{fixture:"same"},{fixture:"same"}).fixture;
  for (let renderIndex=0;renderIndex<3;renderIndex++) {
    const ui=render(value);
    expect(ui.card).toContain("Таймаут");expect(ui.active).toContain("Таймаут выбранного сервера");
    expect(ui.card).not.toContain("server-card-delay fast");expect(ui.active).not.toContain("56 мс");
  }
  expect(value.pending).toBe(true);
  expect(mergeLatency(value,ok)).toEqual(value);
  const current=selectedLatency({ATLAS:{now:"fixture"},fixture:{alive:true,history:[{time:new Date().toISOString(),delay:56}],atlasHealth:value}})!;
  expect(render(current).active).toContain("Таймаут выбранного сервера");
  const recovered=mergeLatency(current,{status:"ok",delay:81,attempts:1,startedAt:now+1,measuredAt:now+2,operationId:3,pending:false});
  expect(render(recovered).active).toContain("81 мс");
  expect(mergeLatency(recovered,timeout)).toEqual(recovered);
  expect(retainNodeLatencies({fixture:recovered},{fixture:"old"},{fixture:"new"})).toEqual({});
});

test("transport failure использует существующую ошибку проверки, не выдумывает timeout", () => {
  const ui=render({status:"error",errorKind:"proxy_or_transport",error:"HTTP 503",delay:null,attempts:1});
  expect(ui.card).toContain("Ошибка проверки");expect(ui.active).toContain("Ошибка проверки");
  expect(ui.card).not.toContain("Таймаут");expect(ui.active).not.toContain("Таймаут");
});
