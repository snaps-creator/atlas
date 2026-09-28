import { expect, it, vi } from "vitest";
const mock = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ isTauri: () => true, invoke: mock.invoke }));
import { request } from "./api";

it("shares concurrent proxy reads and retries after failure without caching an error", async () => {
  let reject!: (reason: Error) => void;
  mock.invoke.mockReturnValueOnce(new Promise((_, fail) => { reject = fail; }));
  const requests = Array.from({ length: 20 }, () => request("proxies"));
  const outcomes = Promise.allSettled(requests);
  expect(mock.invoke).toHaveBeenCalledTimes(1);
  reject(new Error("controller unavailable"));
  expect((await outcomes).every(result => result.status === "rejected")).toBe(true);
  mock.invoke.mockResolvedValueOnce({ proxies: {} });
  await expect(request("proxies")).resolves.toEqual({ proxies: {} });
  expect(mock.invoke).toHaveBeenCalledTimes(2);
});
