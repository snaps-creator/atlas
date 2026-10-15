import { expect, test } from "vitest";
import { resolveServer, selectedLatency } from "./ActiveServer";

test("active view uses certified evidence, ignores unrelated history and expires it", () => {
  const measuredAt=Date.now();
  const history=[{time:new Date(measuredAt).toISOString(),delay:56}];
  const atlasHealth={status:"unreachable" as const,delay:null,attempts:1,measuredAt};
  expect(selectedLatency({ATLAS:{now:"AUTO"},AUTO:{now:"Paris"},Paris:{alive:true,history,atlasHealth}}))
    .toMatchObject({status:"unreachable",delay:null});
  expect(selectedLatency({ATLAS:{now:"Paris"},Paris:{alive:true,history}})).toBeUndefined();
  expect(selectedLatency({ATLAS:{now:"Paris"},Paris:{atlasHealth:{...atlasHealth,status:"ok",delay:56}}})).toMatchObject({status:"ok",delay:56});
  expect(selectedLatency({ATLAS:{now:"Paris"},Paris:{atlasHealth:{...atlasHealth,status:"ok",delay:56,measuredAt:measuredAt-10_001}}})).toMatchObject({status:"stale",delay:null});
  expect(selectedLatency({ATLAS:{now:"ATLAS"}})).toBeUndefined();
});

test("resolves the actual node through automatic groups", () => {
  expect(resolveServer({ ATLAS: { now: "AUTO" }, AUTO: { now: "Prague" }, Prague: {} })).toBe("Prague");
  expect(resolveServer({ ATLAS: { now: "Paris" }, Paris: {} })).toBe("Paris");
});
test("rejects missing nodes and cyclic groups", () => {
  expect(resolveServer({ ATLAS: { now: "AUTO" }, AUTO: { now: "ATLAS" } })).toBeNull();
  expect(resolveServer({ ATLAS: { now: "missing" } })).toBeNull();
});
