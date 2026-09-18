import { useEffect, useState } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { request } from "./api";
type Application = {
  name: string;
  process: string;
  path: string;
  running: boolean;
  icon: string | null;
};
export function AppPicker({
  value,
  onChange,
}: {
  value: string;
  onChange: (v: string) => void;
}) {
  const [apps, setApps] = useState<Application[]>([]),
    [query, setQuery] = useState(""),
    [source, setSource] = useState("running"),
    [error, setError] = useState(""),
    [loading, setLoading] = useState(true);
  useEffect(() => {
    let alive = true;
    void request<Application[]>("applications")
      .then((v) => {
        if (alive) setApps(v);
      })
      .catch((e) => {
        if (alive) setError(String(e));
      })
      .finally(() => {
        if (alive) setLoading(false);
      });
    return () => {
      alive = false;
    };
  }, []);
  useEffect(() => {
    let alive = true;
    let cleanup: (() => void) | undefined;
    void getCurrentWebview()
      .onDragDropEvent((event) => {
        if (event.payload.type === "drop") {
          const names = event.payload.paths
            .filter((p) => p.toLowerCase().endsWith(".exe"))
            .map((p) => p.split(/[\\/]/).pop()!);
          onChange(
            [
              ...new Set([...value.split(/\r?\n/), ...names].filter(Boolean)),
            ].join("\n"),
          );
        }
      })
      .then((fn) => {
        if (alive) cleanup = fn;
        else fn();
      })
      .catch((e) => setError(String(e)));
    return () => {
      alive = false;
      cleanup?.();
    };
  }, [value, onChange]);
  const selected = value.split(/\r?\n/).map((v) => v.toLowerCase());
  return (
    <div className="app-picker">
      <div className="toolbar">
        <button
          className={source === "running" ? "selected" : ""}
          onClick={() => setSource("running")}
        >
          Запущенные
        </button>
        <button
          className={source === "all" ? "selected" : ""}
          onClick={() => setSource("all")}
        >
          Все приложения
        </button>
        <input
          placeholder="Поиск приложений"
          aria-label="Поиск приложений"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
        />
      </div>
      {loading && <p>Загрузка приложений…</p>}
      {error && <p role="alert">{error}</p>}
      <div className="app-picker-list">
        {apps
          .filter(
            (a) =>
              (source !== "running" || a.running) &&
              `${a.name} ${a.process}`
                .toLowerCase()
                .includes(query.toLowerCase()),
          )
          .map((a) => (
            <label key={a.path} className="app-choice">
              <input
                type="checkbox"
                checked={selected.includes(a.process.toLowerCase())}
                onChange={(e) =>
                  onChange(
                    e.target.checked
                      ? [value, a.process].filter(Boolean).join("\n")
                      : value
                          .split(/\r?\n/)
                          .filter(
                            (v) => v.toLowerCase() !== a.process.toLowerCase(),
                          )
                          .join("\n"),
                  )
                }
              />
              {a.icon ? (
                <img src={a.icon} width={24} height={24} alt="" />
              ) : (
                <span aria-hidden="true">▣</span>
              )}
              <span>
                {a.name}
                <small>{a.process}</small>
              </span>
            </label>
          ))}
      </div>
      <p className="muted">Перетащите EXE или вставьте путь ниже</p>
    </div>
  );
}
