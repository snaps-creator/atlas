import { describe, expect, it, vi } from "vitest";
import { unavailableProtection, protectionLabel, connectionProtection, afterConnectionReady } from "./protection";

it("waits for Connected and cancels the delayed probe on disconnect", () => {
  vi.useFakeTimers();
  try {
    const inspect = vi.fn();
    expect(connectionProtection(true, "Connecting")?.secure).toBeNull();
    expect(connectionProtection(false, "Connected")?.secure).toBe(false);
    expect(connectionProtection(true, "Connected")).toBeNull();
    const cancelPending = afterConnectionReady(false, inspect);
    vi.advanceTimersByTime(6000);
    expect(inspect).not.toHaveBeenCalled();
    cancelPending();
    const cancel = afterConnectionReady(true, inspect);
    vi.advanceTimersByTime(4999);
    expect(inspect).not.toHaveBeenCalled();
    cancel();
    vi.advanceTimersByTime(1);
    expect(inspect).not.toHaveBeenCalled();
    afterConnectionReady(true, inspect);
    vi.advanceTimersByTime(5000);
    expect(inspect).toHaveBeenCalledTimes(1);
  } finally { vi.useRealTimers(); }
});

describe("protection uncertainty", () => {
  it("does not turn a busy check into an exposure or a fresh success", () => {
    const result = unavailableProtection({ secure: true, detail: "ok", checkedAt: 123 }, "Atlas занят, повторите операцию");
    expect(result.secure).toBeNull();
    expect(result.checkedAt).toBe(0);
    expect(result.lastConfirmedAt).toBe(123);
    expect(protectionLabel(result)).toBe("Защита не проверена");
    expect(unavailableProtection(result, "Сетевая служба занята").lastConfirmedAt).toBe(123);
  });
  it("does not invent a successful check after a failure", () => {
    const result = unavailableProtection({ secure: false, detail: "disconnected", checkedAt: 123 }, "timeout");
    expect(result.lastConfirmedAt).toBeUndefined();
    expect(result.secure).toBeNull();
    expect(protectionLabel({ secure: false, detail: "failed", checkedAt: 124 })).toBe("Данные не защищены");
  });
});
