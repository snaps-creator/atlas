import { describe, expect, it } from "vitest";
import { trafficRate } from "./traffic";

describe("traffic rate", () => {
  it("does not treat session totals as speed on the first poll", () => {
    expect(
      trafficRate(null, { time: 1000, up: 500000, down: 900000 }),
    ).toBeNull();
  });
  it("accounts for delayed polls", () => {
    expect(
      trafficRate(
        { time: 1000, up: 100, down: 200 },
        { time: 5000, up: 500, down: 1000 },
      ),
    ).toEqual({ time: 5000, up: 100, down: 200 });
  });
  it("handles a core counter reset and a clock adjustment", () => {
    const before = { time: 5000, up: 10000, down: 90000 };
    expect(trafficRate(before, { time: 6000, up: 0, down: 0 })).toEqual({
      time: 6000,
      up: 0,
      down: 0,
    });
    expect(
      trafficRate(before, { time: 4000, up: 20000, down: 100000 }),
    ).toBeNull();
  });
});
