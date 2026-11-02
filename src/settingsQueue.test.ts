import { expect, test, vi } from "vitest";
import { SettingsQueue } from "./settingsQueue";
import type { Settings, Snapshot } from "./types";
const initial = () => ({ revision: 1, settings: { theme: "dark", startup: { autoConnect: false, launchWithWindows: false }, selected: "a" } }) as unknown as Snapshot;
test("rapid toggles remain visible and preserve independent settings and concurrent selection", async () => {
  let state=initial(), release!:()=>void;
  const blocked=new Promise<void>(resolve=>release=resolve);
  const write=vi.fn(async (settings: Settings, revision: number) => {
    if(write.mock.calls.length===1) await blocked;
    expect(revision).toBe(state.revision);
    state={...state,settings,revision:revision+1}; return state;
  });
  const queue=new SettingsQueue(async()=>state,write,s=>{
    state=s;
    // A separate committed selection between the two settings writes.
    if(s.revision===2) state={...state,revision:3,settings:{...state.settings,selected:"b"}};
  },()=>{});
  const first=queue.push(state.settings,{...state.settings,startup:{...state.settings.startup,autoConnect:true}});
  const shown=queue.overlay(state.settings);
  const second=queue.push(shown,{...shown,startup:{...shown.startup,launchWithWindows:true}});
  expect(queue.overlay(state.settings).startup).toEqual({autoConnect:true,launchWithWindows:true});
  await Promise.resolve();expect(write).toHaveBeenCalledTimes(1);
  release();await first;
  await second;
  expect(state.settings.startup).toEqual({autoConnect:true,launchWithWindows:true});
  expect(state.settings.selected).toBe("b");
  expect(write).toHaveBeenCalledTimes(2);
});
test("failure rolls back only its toggle and does not stall the next save",async()=>{
  let state=initial();const write=vi.fn().mockRejectedValueOnce(Error("disk failure")).mockImplementation(async(settings:Settings,revision:number)=>({...state,settings,revision:revision+1}));
  const queue=new SettingsQueue(async()=>state,write,s=>{state=s;},()=>{});
  const first=queue.push(state.settings,{...state.settings,theme:"light"});
  const shown=queue.overlay(state.settings);
  const second=queue.push(shown,{...shown,startup:{...shown.startup,autoConnect:true}});
  await expect(first).rejects.toThrow("disk failure");await second;
  expect(queue.overlay(state.settings).theme).toBe("dark");expect(state.settings.startup.autoConnect).toBe(true);
});
test("explicit revision conflict rebases once; transport errors are not replayed",async()=>{
  let state=initial();const write=vi.fn().mockRejectedValueOnce(Error("Настройки уже изменились. Обновите страницу"))
    .mockImplementation(async(settings:Settings,revision:number)=>({...state,settings,revision:revision+1}));
  const queue=new SettingsQueue(async()=>state,write,s=>{state=s;},()=>{});
  await queue.push(state.settings,{...state.settings,theme:"light"});expect(write).toHaveBeenCalledTimes(2);
  write.mockRejectedValueOnce(Error("transport timeout"));
  await expect(queue.push(state.settings,{...state.settings,theme:"dark"})).rejects.toThrow("transport timeout");
  expect(write).toHaveBeenCalledTimes(3);expect(state.settings.theme).toBe("light");
});
