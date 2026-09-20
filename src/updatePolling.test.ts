import { afterEach, describe, expect, it, vi } from "vitest";
import { startUpdatePolling, UPDATE_CHECK_INTERVAL_MS } from "./updatePolling";

afterEach(() => { vi.clearAllTimers(); vi.useRealTimers(); });
describe("update polling", () => {
  it("checks at startup, retries after failures every six hours", async () => {
    vi.useFakeTimers();
    const check = vi.fn().mockRejectedValueOnce(new Error("offline")).mockResolvedValue(null);
    const error = vi.fn();
    const stop = startUpdatePolling(check, vi.fn(), error);
    await vi.advanceTimersByTimeAsync(0);
    expect(error).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(UPDATE_CHECK_INTERVAL_MS - 1);
    expect(check).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(1);
    expect(check).toHaveBeenCalledTimes(2);
    stop();
  });
  it("does not overlap requests and stops after finding an update", async () => {
    vi.useFakeTimers();
    let resolve!: (value: string | null) => void;
    const check = vi.fn(() => new Promise<string | null>(done => { resolve = done; }));
    const found = vi.fn();
    startUpdatePolling(check, found, vi.fn());
    await vi.advanceTimersByTimeAsync(UPDATE_CHECK_INTERVAL_MS * 2);
    expect(check).toHaveBeenCalledTimes(1);
    resolve("new-version");
    await vi.advanceTimersByTimeAsync(0);
    expect(found).toHaveBeenCalledWith("new-version");
    await vi.advanceTimersByTimeAsync(UPDATE_CHECK_INTERVAL_MS * 2);
    expect(check).toHaveBeenCalledTimes(1);
  });
  it("cancels the first StrictMode effect before making a request", async () => {
    vi.useFakeTimers();
    const check = vi.fn().mockResolvedValue(null);
    startUpdatePolling(check, vi.fn(), vi.fn())();
    const stop = startUpdatePolling(check, vi.fn(), vi.fn());
    await vi.advanceTimersByTimeAsync(0);
    expect(check).toHaveBeenCalledTimes(1);
    stop();
  });
  it("ignores a response after cleanup", async () => {
    vi.useFakeTimers();
    let resolve!: (value: string | null) => void;
    const found = vi.fn();
    const stop = startUpdatePolling(() => new Promise<string | null>(done => { resolve = done; }), found, vi.fn());
    await vi.advanceTimersByTimeAsync(0);
    stop();
    resolve("new-version");
    await vi.advanceTimersByTimeAsync(0);
    expect(found).not.toHaveBeenCalled();
  });
});
