import { beforeEach, expect, it, vi } from "vitest";
const mocks=vi.hoisted(()=>({invoke:vi.fn(),isTauri:vi.fn(()=>true)}));
vi.mock("@tauri-apps/api/core",()=>mocks);
import {diagnosticEvent} from "./diagnosticEvents";
beforeEach(()=>{mocks.invoke.mockReset();mocks.isTauri.mockReturnValue(true);});
it("bounds diagnostic detail and never sends arbitrary objects",async()=>{
  await diagnosticEvent("updater","check_failed","x".repeat(9000));
  expect(mocks.invoke.mock.calls[0][1].payload.detail).toHaveLength(4096);
});
it("a diagnostic IPC failure does not recurse or reject the user operation",async()=>{
  mocks.invoke.mockRejectedValue(new Error("pipe closed"));
  await expect(diagnosticEvent("updater","update_failed","error")).resolves.toBeUndefined();
  expect(mocks.invoke).toHaveBeenCalledTimes(1);
});
