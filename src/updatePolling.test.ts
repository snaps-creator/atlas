import { afterEach, describe, expect, it, vi } from "vitest";
import { startUpdatePolling, UPDATE_CHECK_INTERVAL_MS } from "./updatePolling";

afterEach(() => { vi.clearAllTimers(); vi.useRealTimers(); });
describe("update polling", () => {
  it("allows manual retry after an error and reports current version", async () => {
    vi.useFakeTimers();
    const check = vi.fn().mockRejectedValueOnce(new Error("offline")).mockResolvedValue(null);
    const current = vi.fn();
    const started = vi.fn();
    const polling = startUpdatePolling(check, vi.fn(), vi.fn(), { onStart: started, onCurrent: current });
    await vi.advanceTimersByTimeAsync(0);
    await polling.checkNow();
    expect(check).toHaveBeenCalledTimes(2);
    expect(started).toHaveBeenCalledTimes(2);
    expect(current).toHaveBeenCalledTimes(1);
    polling();
  });
  it("shares the in-flight guard between manual and scheduled checks", async () => {
    vi.useFakeTimers();
    let resolve!: (value: string | null) => void;
    const check = vi.fn(() => new Promise<string | null>(done => { resolve = done; }));
    const polling = startUpdatePolling(check, vi.fn(), vi.fn());
    await vi.advanceTimersByTimeAsync(0);
    await polling.checkNow();
    expect(check).toHaveBeenCalledTimes(1);
    resolve(null);
    await vi.advanceTimersByTimeAsync(0);
    polling();
  });
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
  it("does not overlap requests and keeps checking after finding an update", async () => {
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
    expect(check).toHaveBeenCalledTimes(2);
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
  it("replaces an offered update without installing the intermediate version", async () => {
    vi.useFakeTimers();
    const check = vi.fn().mockResolvedValueOnce("2.2.1").mockResolvedValueOnce("2.2.2").mockResolvedValue(null);
    const found = vi.fn(), current = vi.fn();
    const polling = startUpdatePolling(check, found, vi.fn(), { onCurrent: current });
    await vi.advanceTimersByTimeAsync(0);
    await polling.checkNow();
    expect(found.mock.calls.map(c => c[0])).toEqual(["2.2.1", "2.2.2"]);
    await polling.checkNow();
    expect(current).toHaveBeenCalledTimes(1);
    polling();
  });
  it("does not replace an installer while a download is active", async () => {
    vi.useFakeTimers(); let busy = true;
    const check = vi.fn().mockResolvedValue("latest");
    const polling = startUpdatePolling(check, vi.fn(), vi.fn(), { shouldCheck: () => !busy });
    await vi.advanceTimersByTimeAsync(0); await polling.checkNow();
    expect(check).not.toHaveBeenCalled();
    busy = false; await polling.checkNow(); expect(check).toHaveBeenCalledTimes(1); polling();
  });

});
