import { expect, test } from "vitest";
import { resolveServer } from "./ActiveServer";
test("resolves the actual node through automatic groups", () => {
  expect(resolveServer({ ATLAS: { now: "AUTO" }, AUTO: { now: "Prague" }, Prague: {} })).toBe("Prague");
  expect(resolveServer({ ATLAS: { now: "Paris" }, Paris: {} })).toBe("Paris");
});
test("rejects missing nodes and cyclic groups", () => {
  expect(resolveServer({ ATLAS: { now: "AUTO" }, AUTO: { now: "ATLAS" } })).toBeNull();
  expect(resolveServer({ ATLAS: { now: "missing" } })).toBeNull();
});
