import { describe, expect, it } from "vitest";
import { unavailableProtection, protectionLabel } from "./protection";

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
