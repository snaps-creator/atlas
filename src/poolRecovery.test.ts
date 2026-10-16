import { describe, expect, it } from "vitest";
import { exhausted, poolVerdict, observedHealth } from "./usePoolRecovery";
import { LATENCY_FRESH_MS } from "./latency";
it("retains minute-cadence pool evidence but never extends its expiry", () => {
  const measuredAt=1000;
  const proxies={ATLAS:{now:"node"},node:{atlasHealth:{status:"ok" as const,delay:56,attempts:1,measuredAt}}};
  expect(observedHealth(["node"],proxies,LATENCY_FRESH_MS,measuredAt+60_000).phase).toBe("healthy");
  expect(observedHealth(["node"],proxies,LATENCY_FRESH_MS,measuredAt+LATENCY_FRESH_MS).phase).toBe("idle");
});
it("never conceals a failed selected node behind healthy alternatives or stale history", () => {
  const now = Date.now();
  const p = {ATLAS:{now:"bad"},bad:{atlasHealth:{status:"unreachable" as const,delay:null,attempts:1,measuredAt:now},alive:false,history:[{delay:0,time:new Date(now).toISOString()}]},good:{atlasHealth:{status:"ok" as const,delay:30,attempts:1,measuredAt:now},alive:true,history:[{delay:30,time:new Date(now).toISOString()}]}};
  expect(observedHealth(["bad","good"],p,30000,now).phase).toBe("degraded");
  expect(observedHealth(["bad","good"],p,30000,now+60000).phase).toBe("idle");
});
describe("shared subscription pool", () => {
  it("never promotes legacy secondary history into certified primary health", () => {
    const now = Date.now();
    const history = [{ delay: 25, time: new Date(now).toISOString() }];
    const proxies = {
      ATLAS: { now: "node" }, node: { extra: {
        "http://cp.cloudflare.com/generate_204": { alive: false, history },
        "https://www.google.com/generate_204": { alive: true, history },
      } },
    };
    expect(observedHealth(["node"], proxies, 30000, now).phase).toBe("idle");
    proxies.node.extra["https://www.google.com/generate_204"].alive = false;
    expect(observedHealth(["node"], proxies, 30000, now).phase).toBe("idle");
    proxies.node.extra["https://www.google.com/generate_204"] = {
      alive: true, history: [{ delay: 25, time: new Date(now - 60000).toISOString() }],
    };
    expect(observedHealth(["node"], proxies, 30000, now).phase).toBe("idle");
  });
  it("does not confuse untested, missing, or empty nodes with total failure", () => {
    expect(exhausted([], {})).toBe(false);
    expect(exhausted(["a", "b"], { a: { alive: false, history: [{ delay: 0 }] } })).toBe(false);
    expect(exhausted(["a"], { a: { alive: false } })).toBe(false);
  });
  it("includes every subscription and accepts a working node from either", () => {
    expect(exhausted(["sub1-a", "sub2-b"], {
      "sub1-a": { alive: false, history: [{ delay: 0 }] },
      "sub2-b": { alive: true, history: [{ delay: 42 }] },
    })).toBe(false);
    expect(poolVerdict({ "sub2-b": 42 }).phase).toBe("healthy");
    expect(poolVerdict({ local: 0 }).phase).toBe("healthy");
    expect(poolVerdict({}, ["secondary-only"]).phase).toBe("healthy");
  });
  it("reports total timeouts without claiming provider fault", () => {
    expect(exhausted(["a"], { a: { alive: false, history: [{ delay: 0 }] } })).toBe(true);
    expect(poolVerdict({}).phase).toBe("all_timeout");
    expect(poolVerdict({}).text).toContain("не установлена");
  });
});
