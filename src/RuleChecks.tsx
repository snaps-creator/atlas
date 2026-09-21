import { useRef, useState } from "react";
import { request } from "./api";
import { stringify } from "yaml";
import type { Settings, Route, Snapshot } from "./types";
import { testPool } from "./latency";
import { routeName } from "./RuleEditor";
type Result = {
  status: string;
  delay: number | null;
  detail: string;
  logErrors: number;
  suggestions: { domain: string; reason: string }[];
};
type Check = Result & {
  key: string;
  value: string;
  route: Route;
  selected: string[];
};
export function RuleChecks({
  settings,
  connected,
  visible,
  refresh,
}: {
  settings?: Settings;
  connected: boolean;
  visible: boolean;
  refresh: () => Promise<void>;
}) {
  const active = useRef(false);
  const generation = useRef(0);
  const [checking, setChecking] = useState(false);
  const [checkedAt, setCheckedAt] = useState<string | null>(null);
  const [checks, setChecks] = useState<Record<string, Check>>({});
  const [error, setError] = useState("");
  async function checkRules() {
    if (!connected || active.current || (settings?.routingMode && settings.routingMode !== "rule")) return;
    active.current = true;
    const run = ++generation.current;
    setChecking(true);
    setChecks({});
    setError("");
    setCheckedAt(new Date().toLocaleTimeString("ru-RU"));
    try {
      const snapshot = await request<Snapshot>("snapshot");
      if (generation.current !== run) return;
      const rules = new Map<string, Check>();
      for (const group of snapshot.settings.groups.filter((g) => g.enabled)) {
        for (const rule of group.rules) {
          const key = rule.kind + ":" + rule.value;
          rules.set(key, {
            key,
            value: rule.value,
            route: group.route,
            status: "queued",
            delay: null,
            detail: "Ожидает проверки",
            logErrors: 0,
            suggestions: [],
            selected: [],
          });
        }
      }
      setChecks(Object.fromEntries(rules));
      await testPool(
        [...rules.values()],
        async (c) => {
          if (generation.current !== run) return;
          setChecks((old) => ({
            ...old,
            [c.key]: { ...c, status: "testing", detail: "Проверка…" },
          }));
          try {
            const result = await request<Result>("rule_probe", { key: c.key });
            if (generation.current !== run) return;
            setChecks((old) => ({
              ...old,
              [c.key]: {
                ...c,
                ...result,
                selected: result.suggestions.map((s) => s.domain),
              },
            }));
          } catch (e) {
            if (generation.current !== run) return;
            setChecks((old) => ({
              ...old,
              [c.key]: { ...c, status: "error", detail: String(e) },
            }));
          }
        },
        2,
      );
    } catch (e) {
      if (generation.current === run) setError(String(e));
    } finally {
      if (generation.current === run) {
        active.current = false;
        setChecking(false);
      }
    }
  }
  async function add(c: Check) {
    if (!settings) return;
    setError("");
    try {
      await request("rules_apply", {
        text: stringify({
          version: 1,
          "default-route": settings.defaultRoute.toLowerCase(),
          rules: c.selected.map((domain) => ({
            "domain-suffix": domain,
            route: c.route.toLowerCase(),
          })),
        }),
        replace: false,
        choices: {},
      });
      setChecks((old) => ({
        ...old,
        [c.key]: { ...old[c.key], suggestions: [], selected: [] },
      }));
      await refresh();
    } catch (e) {
      setError(String(e));
    }
  }
  const entries = Object.values(checks);
  if (!visible) return null;
  if (settings?.routingMode && settings.routingMode !== "rule") {
    return visible ? <p className="muted">Проверка правил доступна в режиме «Правила». Переключите режим на Главной.</p> : null;
  }
  return (
    <section className="rule-checks">
      <h2>Проверка правил</h2>
      <button
        disabled={
          !connected ||
          checking ||
          !settings?.groups.some((g) => g.enabled && g.rules.length)
        }
        onClick={() => void checkRules()}
      >
        {checking ? "Проверка…" : "Проверить правила"}
      </button>
      <button
        disabled={!checking && !checkedAt && !error}
        onClick={() => {
          generation.current++;
          active.current = false;
          setChecking(false);
          setChecks({});
          setCheckedAt(null);
          setError("");
        }}
        style={{ marginLeft: 8 }}
      >Очистить</button>
      {checkedAt && <p>Проверка по запросу в {checkedAt}</p>}
      {error && (
        <p role="alert" className="error">
          {error}
        </p>
      )}
      {entries.map((c) => (
        <details
          key={c.key}
          open={
            c.status === "error" ||
            c.status === "warning" ||
            c.suggestions.length > 0
          }
        >
          <summary>
            <strong>{c.value}</strong> ·{" "}
            {c.status === "testing"
              ? "Проверка…"
              : c.delay !== null
                ? `${c.delay} мс`
                : c.key.startsWith("PROCESS-")
                  ? "Снимок соединений"
                  : "—"}{" "}
            · {c.detail}
          </summary>
          <p>Связанных ошибок ядра: {c.logErrors}</p>
          {c.suggestions.length > 0 && (
            <>
              <p>
                Домены, которые могут влиять на работу сайта. Выберите нужные:
              </p>
              {c.suggestions.map((s) => (
                <label className="dependency-choice" key={s.domain}>
                  <input
                    type="checkbox"
                    checked={c.selected.includes(s.domain)}
                    onChange={(e) =>
                      setChecks((old) => ({
                        ...old,
                        [c.key]: {
                          ...old[c.key],
                          selected: e.target.checked
                            ? [...old[c.key].selected, s.domain]
                            : old[c.key].selected.filter((v) => v !== s.domain),
                        },
                      }))
                    }
                  />
                  <span>
                    <strong>{s.domain}</strong>
                    <small>{s.reason}</small>
                  </span>
                </label>
              ))}
              <button disabled={!c.selected.length} onClick={() => void add(c)}>
                Добавить выбранные → {routeName(c.route)}
              </button>
            </>
          )}
        </details>
      ))}
    </section>
  );
}
