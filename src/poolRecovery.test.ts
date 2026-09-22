import { describe, expect, it } from "vitest";
import { exhausted, poolVerdict } from "./usePoolRecovery";
describe("shared subscription pool", () => {
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
    expect(poolVerdict({ "sub2-b": 42 }, false).phase).toBe("healthy");
    expect(poolVerdict({ local: 0 }, false).phase).toBe("healthy");
  });
  it("reports total timeouts without claiming provider fault", () => {
    expect(exhausted(["a"], { a: { alive: false, history: [{ delay: 0 }] } })).toBe(true);
    expect(poolVerdict({}, false).phase).toBe("all_timeout");
    expect(poolVerdict({}, false).text).toContain("причина не установлена");
    expect(poolVerdict({}, true).text).toContain("не полностью");
  });
});
