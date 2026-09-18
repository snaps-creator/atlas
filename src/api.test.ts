import { describe, it, expect } from "vitest";
import { bytes, request } from "./api";
describe("display data", () => {
  it("formats real byte counters without fabricating values", () => {
    expect(bytes(0)).toBe("0 B");
    expect(bytes(1024)).toBe("1.0 KB");
    expect(bytes(1048576)).toBe("1.0 MB");
    expect(bytes(1073741824)).toBe("1.00 GB");
  });
  it("fails explicitly when native transport is unavailable", async () => {
    await expect(request("connect")).rejects.toThrow("Windows-приложение");
  });
});
