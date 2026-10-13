import { describe, expect, it } from "vitest";
import { allSubscriptions, validSubscriptionInput, serverSelectionPayload, isServerSelected, isFavorite, toggleFavorite } from "./subscriptionSource";
import type { Server, Subscription } from "./types";

const urlNode: Server = { name: "Paris", type: "vless", probeId: "probe-a", nodeId: "url:one", sourceType: "URL" };
const keyNode: Server = { ...urlNode, nodeId: "key:one", probeId: "probe-b", sourceType: "VLESS" };
const legacyNode: Server = { name: "Legacy", type: "vless", probeId: "old" };

describe("unified subscriptions", () => {
  it("keeps URL, VLESS and legacy subscriptions together, including refresh errors", () => {
    const subscriptions = [
      { id: "old", servers: [legacyNode], error: null },
      { id: "url", source: "URL", servers: [urlNode], error: null },
      { id: "key", source: "VLESS", servers: [keyNode], error: "Refresh failed" },
    ] as Subscription[];
    const visible = allSubscriptions({ subscriptions });
    expect(visible).toBe(subscriptions);
    expect(visible.flatMap(sub => sub.servers)).toEqual([legacyNode, urlNode, keyNode]);
    expect(visible[2].error).toBe("Refresh failed");
  });
  it("handles missing and empty snapshots", () => {
    expect(allSubscriptions()).toEqual([]);
    expect(allSubscriptions(null)).toEqual([]);
    expect(allSubscriptions({ subscriptions: [] })).toEqual([]);
  });
});

describe("subscription ingestion type", () => {
  it.each([
    ["URL", "https://provider.example/sub", true],
    ["URL", "  https://provider.example/sub  ", true],
    ["URL", "http://provider.example/sub", false],
    ["URL", "vless://user@host:443", false],
    ["VLESS", "vless://user@host:443?security=tls#Paris", true],
    ["VLESS", "https://provider.example/sub", false],
    ["VLESS", "vless://host:443", false],
    ["VLESS", "", false],
    ["URL", "invalid", false],
  ] as const)("validates %s input %s", (source, input, valid) => {
    expect(validSubscriptionInput(source, input)).toBe(valid);
  });
});

describe("canonical server selection and favorites", () => {
  it("sends node IDs and automatic sentinels, with a legacy name fallback", () => {
    expect(serverSelectionPayload(urlNode)).toEqual({ nodeId: "url:one" });
    expect(serverSelectionPayload(keyNode)).toEqual({ nodeId: "key:one" });
    expect(serverSelectionPayload("AUTO")).toEqual({ nodeId: "AUTO" });
    expect(serverSelectionPayload("FAILOVER")).toEqual({ nodeId: "FAILOVER" });
    expect(serverSelectionPayload(legacyNode)).toEqual({ name: "Legacy" });
  });
  it("distinguishes equal display names and survives runtime renaming", () => {
    const selection = { selected: "Old runtime name", selectedNodeId: "key:one" };
    expect(isServerSelected(selection, keyNode)).toBe(true);
    expect(isServerSelected(selection, urlNode)).toBe(false);
    expect(isServerSelected({ selected: "Legacy" }, legacyNode)).toBe(true);
    expect(isServerSelected({ selected: "Paris", selectedNodeId: "AUTO" }, keyNode)).toBe(false);
    expect(isServerSelected(undefined, keyNode)).toBe(false);
  });
  it("toggles favorites across both source types without conflating equal names", () => {
    const original = ["unrelated"];
    const favorites = toggleFavorite(toggleFavorite(original, urlNode), keyNode);
    expect(favorites).toEqual(["unrelated", "url:one", "key:one"]);
    expect(original).toEqual(["unrelated"]);
    expect(isFavorite(favorites, { ...urlNode, name: "Renamed" })).toBe(true);
    const remaining = toggleFavorite(favorites, urlNode);
    expect(isFavorite(remaining, urlNode)).toBe(false);
    expect(isFavorite(remaining, keyNode)).toBe(true);
    expect(toggleFavorite([], legacyNode)).toEqual(["Legacy"]);
    expect(toggleFavorite(["Legacy"], legacyNode)).toEqual([]);
  });
});
