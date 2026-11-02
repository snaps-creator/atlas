import { expect, test, vi } from "vitest";
import { watchVisibleServers } from "./visibleServerPriority";

test("viewport hints are bounded, follow sorting/filtering and are cleared on exit", () => {
  let intersect: (entries: { target: Element; isIntersecting: boolean }[]) => void = () => {};
  let mutate: () => void = () => {};
  const observe = vi.fn(), unobserve = vi.fn(), disconnected = vi.fn();
  vi.stubGlobal("IntersectionObserver", class {
    constructor(callback: (entries: unknown[]) => void) { intersect = callback; }
    observe = observe; unobserve = unobserve; disconnect = disconnected;
  });
  vi.stubGlobal("MutationObserver", class {
    constructor(callback: () => void) { mutate = callback; }
    observe = vi.fn(); disconnect = disconnected;
  });
  try {
    let cards = Array.from({ length: 40 }, (_, i) => ({ dataset: { serverName: `node-${i}` } }) as unknown as HTMLElement);
    const root = { querySelectorAll: () => cards } as unknown as Element;
    const publish = vi.fn();const stop = watchVisibleServers(root, publish);
    intersect(cards.map(target => ({ target, isIntersecting: true })));
    expect(publish.mock.lastCall?.[0]).toHaveLength(32);
    const count = publish.mock.calls.length;
    intersect(cards.map(target => ({ target, isIntersecting: true })));
    expect(publish.mock.calls).toHaveLength(count);
    cards = cards.slice(10).reverse();mutate();
    expect(publish.mock.lastCall?.[0][0]).toBe("node-39");expect(unobserve).toHaveBeenCalledTimes(10);
    stop();expect(publish.mock.lastCall?.[0]).toEqual([]);expect(disconnected).toHaveBeenCalledTimes(2);
  } finally { vi.unstubAllGlobals(); }
});
