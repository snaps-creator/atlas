import { expect, test, vi } from "vitest";
import { collectAndDownload, type CaptureReport } from "./lanCapture";
const complete: CaptureReport = { deviceId: "pc", at: 1, captureId: "new", captureRunning: false, captureCompletedAt: 2 };
test("downloads exactly once after the requested capture finishes, never the initial incomplete report", async () => {
  const read = vi.fn().mockResolvedValueOnce({ ...complete, captureRunning: true, captureCompletedAt: 0 })
    .mockResolvedValueOnce(complete).mockResolvedValueOnce({ ...complete, incident: { evidence: "ready" } });
  const save = vi.fn();
  await collectAndDownload(read, save, () => true, async () => {});
  expect(read.mock.calls).toEqual([[true, true], [false, true], [false, false]]);
  expect(save).toHaveBeenCalledTimes(1);
  expect(save.mock.calls[0][0].incident.evidence).toBe("ready");
});
test("old peers, collection failures and a changed capture cannot trigger a download", async () => {
  for (const first of [{ ...complete, captureId: undefined }, { ...complete, captureError: "failed" }]) {
    const save = vi.fn();
    await expect(collectAndDownload(async () => first, save, () => true)).rejects.toThrow();
    expect(save).not.toHaveBeenCalled();
  }
  const save = vi.fn();
  const read = vi.fn().mockResolvedValueOnce(complete).mockResolvedValueOnce({ ...complete, captureId: "other" });
  await expect(collectAndDownload(read, save, () => true)).rejects.toThrow("изменился");
  expect(save).not.toHaveBeenCalled();
});
test("closing access and collection timeout stop polling without saving stale data", async () => {
  const save = vi.fn(), read = vi.fn().mockResolvedValue({ ...complete, captureRunning: true });
  await expect(collectAndDownload(read, save, () => false)).rejects.toThrow("закрыт");
  let time = 0;
  await expect(collectAndDownload(read, save, () => true, async () => { time = 150001; }, () => time)).rejects.toThrow("150 секунд");
  expect(save).not.toHaveBeenCalled();
});
