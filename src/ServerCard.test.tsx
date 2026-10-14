import { expect, test } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { ServerCard } from "./ServerCard";
import type { Latency } from "./latency";

function card(latency?: Latency) {
  return renderToStaticMarkup(<ServerCard server={{name:"Berlin",type:"vless",probeId:"node"}}
    selected={false} favorite={false} disabled={false} testing={false}
    latency={latency} onSelect={() => {}} onFavorite={() => {}} onTest={() => {}} />);
}

test("карточка сохраняет числовой пинг, включая 0 и значения выше 150 мс", () => {
  for (const delay of [0, 80, 210, 1500]) {
    const html = card({status:"ok",delay,attempts:1,measuredAt:1});
    expect(html).toContain(`${delay}<small>мс</small>`);
    expect(html).not.toMatch(/Устарело|выше порога|—|–/);
  }
});

test("карточка без результата не выдумывает пинг и не показывает прочерк", () => {
  expect(card()).toContain("Не проверен");
  const html = card({status:"timeout",delay:null,attempts:1,lastDelay:210});
  expect(html).toContain("Таймаут");
  expect(html).not.toMatch(/210|<small>мс|Устарело|выше порога|—|–/);
});
