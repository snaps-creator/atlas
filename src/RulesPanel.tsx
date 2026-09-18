import { useState } from "react";
import { parseDocument, stringify } from "yaml";
import { request } from "./api";
import { RouteSelect, routeName } from "./RuleEditor";
import { YamlEditor } from "./YamlEditor";
import { AppPicker } from "./AppPicker";
import type { Settings, Group, Route } from "./types";
type Preview = {
  groups: Group[];
  added: number;
  updated: number;
  duplicates: number;
  conflicts: { key: string; value: string; routes: Route[] }[];
};
export function RulesPanel({
  settings: s,
  save,
  refresh,
}: {
  settings: Settings;
  save: (s: Settings) => Promise<void>;
  refresh: () => Promise<void>;
}) {
  const [query, setQuery] = useState(""),
    [selected, setSelected] = useState<string[]>([]),
    [modal, setModal] = useState<
      "yaml" | "import" | "export" | "site" | "app" | null
    >(null),
    [text, setText] = useState(""),
    [route, setRoute] = useState<Route>("PROXY"),
    [error, setError] = useState(""),
    [busy, setBusy] = useState(false),
    [preview, setPreview] = useState<Preview | null>(null),
    [choices, setChoices] = useState<Record<string, Route>>({});
  const rows = s.groups.flatMap((g) =>
    g.rules.map((r, i) => ({ g, r, key: `${g.id}/${i}` })),
  );
  async function perform(f: () => Promise<void>) {
    setBusy(true);
    setError("");
    try {
      await f();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }
  function close() {
    setModal(null);
    setPreview(null);
    setChoices({});
    setError("");
  }
  async function open(mode: typeof modal) {
    setError("");
    setPreview(null);
    setChoices({});
    setText(
      mode === "yaml" || mode === "export"
        ? await request<string>("rules_export")
        : "",
    );
    setModal(mode);
  }
  function edit(
    keys: string[],
    change: "delete" | "disable" | "enable" | Route,
  ) {
    return save({
      ...s,
      groups: s.groups.flatMap((g) =>
        g.rules.flatMap((r, i) => {
          if (!keys.includes(`${g.id}/${i}`))
            return [
              {
                ...g,
                id: g.rules.length === 1 ? g.id : `${g.id}/${i}`,
                rules: [r],
              },
            ];
          if (change === "delete") return [];
          return [
            {
              ...g,
              id: g.rules.length === 1 ? g.id : `${g.id}/${i}`,
              rules: [r],
              enabled:
                change === "disable"
                  ? false
                  : change === "enable"
                    ? true
                    : g.enabled,
              route: ["PROXY", "DIRECT", "BLOCK"].includes(change)
                ? (change as Route)
                : g.route,
            },
          ];
        }),
      ),
    });
  }
  async function review(value = text, replace = modal === "yaml") {
    const p = await request<Preview>("rules_preview", {
      text: value,
      replace,
      choices,
    });
    setPreview(p);
    return p;
  }
  async function apply() {
    const p = await review();
    if (p.conflicts.length) return;
    await request("rules_apply", { text, replace: modal === "yaml", choices });
    await refresh();
    close();
  }
  async function quick() {
    const kind = modal === "app" ? "process" : "domain-suffix";
    const doc = stringify({
      version: 1,
      "default-route": s.defaultRoute.toLowerCase(),
      rules: text
        .split(/\r?\n/)
        .map((v) => v.trim())
        .filter(Boolean)
        .map((v) => ({ [kind]: v, route: route.toLowerCase() })),
    });
    const p = await review(doc, false);
    if (p.conflicts.length) {
      setText(doc);
      setModal("import");
    } else {
      await request("rules_apply", { text: doc, replace: false, choices: {} });
      await refresh();
      close();
    }
  }
  return (
    <>
      <div className="page-head">
        <h1>Правила</h1>
      </div>
      <div className="toolbar actions">
        {(
          [
            ["app", "+ Приложение"],
            ["site", "+ Сайт"],
            ["yaml", "YAML"],
            ["import", "Импорт"],
            ["export", "Экспорт"],
          ] as const
        ).map(([m, label]) => (
          <button
            key={m}
            disabled={busy}
            onClick={() => void perform(() => open(m))}
          >
            {label}
          </button>
        ))}
      </div>
      <input
        aria-label="Поиск правил"
        placeholder="Приложение, домен, IP или маршрут"
        value={query}
        onChange={(e) => setQuery(e.target.value)}
      />
      {selected.length > 0 && (
        <div className="toolbar">
          <span>Выбрано: {selected.length}</span>
          {(["PROXY", "DIRECT", "BLOCK", "disable", "delete"] as const).map(
            (r) => (
              <button
                key={r}
                disabled={busy}
                onClick={() =>
                  void perform(async () => {
                    await edit(selected, r);
                    setSelected([]);
                  })
                }
              >
                {r === "disable"
                  ? "Отключить"
                  : r === "delete"
                    ? "Удалить"
                    : routeName(r)}
              </button>
            ),
          )}
        </div>
      )}
      <div className="list compact-rules">
        {rows
          .filter(({ g, r }) =>
            `${g.name} ${r.value} ${r.kind} ${routeName(g.route)}`
              .toLowerCase()
              .includes(query.toLowerCase()),
          )
          .map(({ g, r, key }) => (
            <div className="rule-table-row" key={key}>
              <input
                type="checkbox"
                aria-label={`Выбрать ${r.value}`}
                checked={selected.includes(key)}
                onChange={(e) =>
                  setSelected(
                    e.target.checked
                      ? [...selected, key]
                      : selected.filter((k) => k !== key),
                  )
                }
              />
              <div className="grow">
                <strong>{r.value}</strong>
                <small>
                  {r.kind === "PROCESS-NAME"
                    ? "Приложение"
                    : r.kind === "PROCESS-DOMAIN"
                      ? "Приложение и домен"
                      : r.kind}
                </small>
              </div>
              <button
                role="switch"
                aria-label={`Включить ${r.value}`}
                aria-checked={g.enabled}
                disabled={busy}
                onClick={() =>
                  void perform(() =>
                    edit([key], g.enabled ? "disable" : "enable"),
                  )
                }
              >
                {g.enabled ? "Включено" : "Отключено"}
              </button>
              <RouteSelect
                value={g.route}
                onChange={(r) => void perform(() => edit([key], r))}
              />
              <button
                aria-label={`Удалить ${r.value}`}
                disabled={busy}
                onClick={() => void perform(() => edit([key], "delete"))}
              >
                ×
              </button>
            </div>
          ))}
      </div>
      {!rows.length && <p className="muted">Правил пока нет</p>}
      <div className="default-route">
        <span>По умолчанию</span>
        {s.mode === "tun" ? (
          <strong>VPN · Вся система</strong>
        ) : (
          <RouteSelect
            value={s.defaultRoute}
            onChange={(defaultRoute) =>
              void perform(() => save({ ...s, defaultRoute }))
            }
          />
        )}
      </div>
      {!modal && error && (
        <div className="error" role="alert">
          {error}
        </div>
      )}
      {modal && (
        <div className="overlay">
          <section
            className="dialog rules-dialog"
            role="dialog"
            aria-modal="true"
            aria-label="Правила"
          >
            <div className="section-head">
              <h2>
                {
                  {
                    yaml: "YAML",
                    import: "Импорт правил",
                    export: "Экспорт правил",
                    site: "Добавить сайты",
                    app: "Добавить приложения",
                  }[modal]
                }
              </h2>
              <button onClick={close} disabled={busy} aria-label="Закрыть">
                ×
              </button>
            </div>
            {modal === "app" && <AppPicker value={text} onChange={setText} />}
            {modal === "yaml" || modal === "import" ? (
              <YamlEditor
                value={text}
                onChange={(v) => {
                  setText(v);
                  setPreview(null);
                  setChoices({});
                  setError("");
                }}
              />
            ) : (
              <textarea
                aria-label={
                  modal === "app" ? "Имена EXE или пути" : "Сайты или YAML"
                }
                readOnly={modal === "export"}
                value={text}
                onChange={(e) => setText(e.target.value)}
                rows={modal === "app" ? 4 : 10}
                placeholder={
                  modal === "app"
                    ? "Telegram.exe\nChatGPT.exe"
                    : "https://chatgpt.com/c/123\nopenai.com"
                }
              />
            )}
            {(modal === "site" || modal === "app") && (
              <RouteSelect value={route} onChange={setRoute} />
            )}
            {error && (
              <div className="error" role="alert">
                {error}
              </div>
            )}
            {preview && (
              <>
                <p>
                  Добавлено: {preview.added} · Обновлено: {preview.updated} ·
                  Дубликатов: {preview.duplicates} · Конфликтов:{" "}
                  {preview.conflicts.length}
                </p>
                {preview.conflicts.map((c) => (
                  <label className="setting-row" key={c.key}>
                    {c.value}
                    <select
                      aria-label={`Маршрут ${c.value}`}
                      value={choices[c.key] ?? ""}
                      onChange={(e) =>
                        setChoices({
                          ...choices,
                          [c.key]: e.target.value as Route,
                        })
                      }
                    >
                      <option value="" disabled>
                        Выберите маршрут
                      </option>
                      {(["PROXY", "DIRECT", "BLOCK"] as Route[]).map((r) => (
                        <option key={r} value={r}>
                          {routeName(r)}
                        </option>
                      ))}
                    </select>
                  </label>
                ))}
              </>
            )}
            <footer>
              {modal === "app" && (
                <button
                  disabled={busy}
                  onClick={() =>
                    void perform(async () => {
                      const paths = await request<string[]>("choose_apps");
                      setText((t) => [t, ...paths].filter(Boolean).join("\n"));
                    })
                  }
                >
                  Выбрать EXE
                </button>
              )}
              {modal === "import" && (
                <>
                  <button
                    disabled={busy}
                    onClick={() =>
                      void perform(async () => {
                        setText(await navigator.clipboard.readText());
                        setPreview(null);
                      })
                    }
                  >
                    Вставить из буфера
                  </button>
                  <button
                    disabled={busy}
                    onClick={() =>
                      void perform(async () => {
                        const value = await request<string | null>(
                          "rules_open_file",
                        );
                        if (value !== null) {
                          setText(value);
                          setPreview(null);
                        }
                      })
                    }
                  >
                    Открыть YAML
                  </button>
                </>
              )}
              {(modal === "yaml" || modal === "export") && (
                <button
                  disabled={busy}
                  onClick={() =>
                    void perform(() => navigator.clipboard.writeText(text))
                  }
                >
                  Скопировать
                </button>
              )}
              {modal === "export" && (
                <button
                  disabled={busy}
                  onClick={() =>
                    void perform(async () => {
                      await request("rules_save_file");
                    })
                  }
                >
                  Сохранить YAML
                </button>
              )}
              {modal === "yaml" && (
                <button
                  disabled={busy}
                  onClick={() =>
                    void perform(async () => {
                      const doc = parseDocument(text);
                      if (doc.errors.length)
                        throw new Error(
                          `Некорректный YAML: строка ${doc.errors[0].linePos?.[0].line ?? "—"}`,
                        );
                      setText(doc.toString());
                    })
                  }
                >
                  Форматировать
                </button>
              )}
              {(modal === "yaml" || modal === "import") && (
                <>
                  <button
                    disabled={busy}
                    onClick={() =>
                      void perform(async () => {
                        await review();
                      })
                    }
                  >
                    Проверить
                  </button>
                  <button
                    className="primary"
                    disabled={busy}
                    onClick={() => void perform(apply)}
                  >
                    Применить
                  </button>
                </>
              )}
              {(modal === "site" || modal === "app") && (
                <button
                  className="primary"
                  disabled={busy || !text.trim()}
                  onClick={() => void perform(quick)}
                >
                  Добавить
                </button>
              )}
              <button disabled={busy} onClick={close}>
                Отменить
              </button>
            </footer>
          </section>
        </div>
      )}
    </>
  );
}
