import { expect, test } from "vitest";
import { resolveServer, selectedLatency } from "./ActiveServer";

test("selected health follows AUTO URL rather than a successful unrelated check", () => {
  const history = [{time:"2026-09-22T12:00:00Z",delay:42}];
  expect(selectedLatency({ATLAS:{now:"AUTO"}, AUTO:{now:"Paris",testUrl:"https://auto.test"},
    Paris:{alive:true,history,extra:{"https://auto.test":{alive:false,history:[{...history[0],delay:0}]}}}}))
    .toMatchObject({status:"unreachable",delay:null});
  expect(selectedLatency({ATLAS:{now:"Paris"},Paris:{alive:true,history}})).toMatchObject({status:"ok",delay:42});
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
