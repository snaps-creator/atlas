import { expect, test, vi } from "vitest";
import { matchingLatencies, startLatencyPolling, type LatencySnapshot } from "./latencyPolling";
const value:LatencySnapshot={generation:1,pending:[],identities:{a:"v1"},results:{a:{status:"ok",delay:1501,attempts:1,measuredAt:1000}}};
test("startup and several timer cycles deliver values without navigation or manual work", async()=>{
  vi.useFakeTimers();const read=vi.fn().mockResolvedValue(value),publish=vi.fn();
  const polling=startLatencyPolling(read,publish);
  try {await vi.advanceTimersByTimeAsync(65000);expect(read).toHaveBeenCalledTimes(66);expect(publish).toHaveBeenLastCalledWith(value);}
  finally {polling.stop();vi.useRealTimers();}
});
test("navigation and streamed results coalesce reads and recover after IPC failure",async()=>{
  vi.useFakeTimers();let complete!:(v:LatencySnapshot)=>void;
  const read=vi.fn().mockImplementationOnce(()=>new Promise(resolve=>complete=resolve)).mockRejectedValueOnce(Error("IPC")).mockResolvedValue(value);
  const publish=vi.fn(),polling=startLatencyPolling(read,publish);
  try {for(let i=0;i<100;i++)void polling.wake();expect(read).toHaveBeenCalledTimes(1);
    complete(value);await vi.advanceTimersByTimeAsync(3000);expect(publish).toHaveBeenLastCalledWith(value);
    expect(matchingLatencies(value,{a:"v2"})).toEqual({});expect(matchingLatencies(value,{a:"v1"}).a.delay).toBe(1501);
  } finally {polling.stop();expect(vi.getTimerCount()).toBe(0);vi.useRealTimers();}
});
test("unmounted UI cannot accept a late response",async()=>{
  let complete!:(v:LatencySnapshot)=>void;const publish=vi.fn();
  const polling=startLatencyPolling(()=>new Promise(resolve=>complete=resolve),publish);
  polling.stop();complete(value);await Promise.resolve();expect(publish).not.toHaveBeenCalled();
});
