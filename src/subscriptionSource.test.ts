import { describe, expect, it } from "vitest";
import { activeSubscriptions } from "./subscriptionSource";
import type { Settings, Subscription } from "./types";

describe("subscription sources", () => {
  it("keeps legacy sources in URL and isolates errors and servers", () => {
    const subscriptions = [
      { id: "old", error: null, servers: [{ name: "A" }] },
      { id: "vless", source: "VLESS", error: "Download failed", servers: [{ name: "B" }] },
    ] as Subscription[];
    const settings = { subscriptions } as Settings;
    expect(activeSubscriptions(settings).map(s => s.id)).toEqual(["old"]);
    expect(activeSubscriptions({ ...settings, activeSource: "VLESS" }).map(s => s.id)).toEqual(["vless"]);
    expect(activeSubscriptions({ ...settings, activeSource: "URL" })[0].error).toBeNull();
    expect(subscriptions).toHaveLength(2);
  });
  it("never falls back to another source when empty", () => {
    expect(activeSubscriptions({ subscriptions: [], activeSource: "VLESS" } as unknown as Settings)).toEqual([]);
    expect(activeSubscriptions()).toEqual([]);
  });
});
