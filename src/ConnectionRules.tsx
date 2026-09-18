import { useState } from "react";
import type { Connection, Route, Settings } from "./types";
import { RouteSelect, routeName } from "./RuleEditor";
export function ConnectionRules({
  connection: c,
  settings: s,
  save,
  onDone,
}: {
  connection: Connection;
  settings: Settings;
  save: (s: Settings) => Promise<void>;
  onDone: () => void;
}) {
  const [route, setRoute] = useState<Route>("PROXY"),
    [error, setError] = useState(""),
    [busy, setBusy] = useState(false);
  async function add(kind: string, value: string) {
    setBusy(true);
    setError("");
    try {
      await save({
        ...s,
        groups: [
          ...s.groups,
          {
            id: crypto.randomUUID(),
            name: value,
            description: "",
            enabled: true,
            route,
            rules: [{ kind, value, noResolve: false }],
          },
        ],
      });
      onDone();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }
  return (
    <>
      <div className="toolbar">
        <RouteSelect value={route} onChange={setRoute} />
        <button
          disabled={busy || !c.metadata.process}
          onClick={() => void add("PROCESS-NAME", c.metadata.process)}
        >
          Добавить приложение в правила
        </button>
        <button
          disabled={busy || !c.metadata.host}
          onClick={() => void add("DOMAIN-SUFFIX", c.metadata.host)}
        >
          Добавить домен в правила
        </button>
      </div>
      {error && <p role="alert">{error}</p>}
    </>
  );
}
export const connectionRoute = (c: Connection) =>
  c.chains.includes("DIRECT")
    ? "Напрямую"
    : c.chains.some((v) => v === "REJECT" || v === "REJECT-DROP")
      ? "Блокировать"
      : "VPN";
