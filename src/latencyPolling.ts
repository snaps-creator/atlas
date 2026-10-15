import type { Latency } from "./latency";
export type LatencySnapshot = {results: Record<string, Latency | null>; identities: Record<string,string>; pending:string[];generation:number;selected?:string|null};
export function matchingLatencies(snapshot: LatencySnapshot, identities: Record<string,string>): Record<string,Latency> {
  return Object.fromEntries(Object.entries(snapshot.results).filter(([name,value]) =>
    value !== null && snapshot.identities[name] === identities[name])
    .map(([name,value])=>[name,{...value,pending:snapshot.pending.includes(name) || value?.pending === true,probeId:snapshot.identities[name]}])) as Record<string,Latency>;
}
// The backend owns probes. These reads only deliver current results, including
// after a hidden WebView resumes; no React timer starts a competing network pass.
export function startLatencyPolling(read:()=>Promise<LatencySnapshot>, publish:(value:LatencySnapshot)=>void) {
  let stopped=false, pending=false, again=false;
  async function wake() {
    if (stopped) return;
    if (pending) {again=true;return;}
    pending=true;
    try {const value=await read();if (!stopped) publish(value);}
    catch { /* The existing application status channel reports IPC failures. */ }
    finally {pending=false;if (again && !stopped) {again=false;void wake();}}
  }
  void wake();
  const timer=setInterval(()=>void wake(),1000);
  return {wake,stop:()=>{stopped=true;clearInterval(timer);}};
}
