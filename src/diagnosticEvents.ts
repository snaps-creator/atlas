import { invoke, isTauri } from "@tauri-apps/api/core";

export async function diagnosticEvent(area: string, stage: string, detail: unknown = ""): Promise<void> {
  if (!isTauri()) return;
  try {
    await invoke("request", {action: "frontend_diagnostic", payload: {
      area: area.slice(0, 80), stage: stage.slice(0, 80), detail: String(detail).slice(0, 4096),
    }});
  } catch { /* A broken IPC channel must not recursively generate diagnostic errors. */ }
}

if (typeof window !== "undefined" && isTauri()) {
  window.addEventListener("error", event => { void diagnosticEvent("frontend", "uncaught_error", event.message); });
  window.addEventListener("unhandledrejection", event => { void diagnosticEvent("frontend", "unhandled_rejection", event.reason); });
}
