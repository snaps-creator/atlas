import { invoke, isTauri } from "@tauri-apps/api/core";
export const native = isTauri();
const reads = new Map<string, Promise<unknown>>();
export async function request<T>(
  action: string,
  payload?: unknown,
): Promise<T> {
  if (!native)
    throw new Error(
      "Откройте Atlas как Windows-приложение. В браузере нет доступа к Mihomo и настройкам Windows.",
    );
  if (action === "proxies" && payload === undefined) {
    const existing = reads.get(action);
    if (existing) return existing as Promise<T>;
    const pending = invoke<T>("request", { action, payload });
    reads.set(action, pending);
    try { return await pending; }
    finally { if (reads.get(action) === pending) reads.delete(action); }
  }
  return invoke<T>("request", { action, payload });
}
export function download(name: string, value: unknown) {
  const blob = new Blob(
    [typeof value === "string" ? value : JSON.stringify(value, null, 2)],
    { type: "text/plain;charset=utf-8" },
  );
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = name;
  a.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}
export function bytes(n: number) {
  if (n < 1024) return `${n} B`;
  if (n < 1048576) return `${(n / 1024).toFixed(1)} KB`;
  if (n < 1073741824) return `${(n / 1048576).toFixed(1)} MB`;
  return `${(n / 1073741824).toFixed(2)} GB`;
}
