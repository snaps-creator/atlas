import { expect, test } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { ServerCard } from "./ServerCard";
import { latencyLabel, type Latency } from "./latency";

test("expired success is never rendered as a current ping", () => {
  const html = renderToStaticMarkup(<ServerCard server={{name:"fixture",type:"vless",probeId:"a"}}
    selected={false} favorite={false} disabled={false} testing={false}
    latency={{status:"ok",delay:43,attempts:1,measuredAt:Date.now()-31000}}
    onSelect={()=>{}} onFavorite={()=>{}} onTest={()=>{}} />);
  expect(html).not.toContain("43<small>");
});

test("missing values have no ping; failures have an explicit label without a cached number", () => {
  expect(latencyLabel()).toBe("");
  expect(latencyLabel({status:"timeout",delay:null,attempts:1})).toBe("Таймаут");
  expect(latencyLabel({status:"error",delay:null,attempts:1})).toBe("Ошибка проверки");
  for (const status of ["queued","testing","stale","not_connected"] as Latency["status"][]) {
    expect(latencyLabel({status,delay:null,attempts:1})).toBe("");
  }
});
test("a changed profile cannot briefly render a previous configuration's ping",()=>{
  const html=renderToStaticMarkup(<ServerCard server={{name:"fixture",type:"vless",probeId:"new"}}
    selected={false} favorite={false} disabled={false} testing={false}
    latency={{status:"ok",delay:43,attempts:1,measuredAt:Date.now(),probeId:"old"}}
    onSelect={()=>{}} onFavorite={()=>{}} onTest={()=>{}} />);
  expect(html).not.toContain("43<small>");
});
