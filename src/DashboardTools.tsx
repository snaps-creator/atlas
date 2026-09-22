import { useEffect, useRef, useState } from "react";
import {
  Activity,
  ArrowDown,
  ArrowUp,
  Globe2,
  RefreshCw,
  SlidersHorizontal,
  Wifi,
} from "lucide-react";
import { request } from "./api";
import type { Settings } from "./types";
import "./dashboard.css";
import type { TrafficSample } from "./traffic";
const megabytes = (n: number) => `${(n / 1_000_000).toFixed(2)} МБ`;
const kilobytesPerSecond = (n: number) =>
  `${(n / 1024).toFixed(1)} КБ/с`;
const sites = [
  ["chatgpt", "ChatGPT"],
  ["grok", "Grok"],
  ["gemini", "Gemini"],
  ["youtube", "YouTube"],
  ["telegram", "Telegram"],
];
const modes = [
  ["rule", "Правила", "Автоматический выбор маршрута по вашим правилам."],
  [
    "global",
    "Глобальный",
    "Весь трафик через выбранный сервер Atlas. Правила не применяются.",
  ],
  [
    "direct",
    "Прямой",
    "Весь трафик напрямую, без VPN. Правила не применяются.",
  ],
] as const;
type SiteResult = {
  ok: boolean;
  ms: number | null;
  status: number | null;
  error?: string;
};
export function DashboardTools({
  connected,
  settings,
  busy,
  save,
  samples,
  traffic,
  memory,
  connections,
  error,
}: {
  connected: boolean;
  settings: Settings | undefined;
  busy: boolean;
  save: (s: Settings) => Promise<void>;
  samples: TrafficSample[];
  traffic: { up: number; down: number };
  memory: number | null;
  connections: number;
  error: string;
}) {
  const [results, setResults] = useState<Record<string, SiteResult>>({});
  const [checking, setChecking] = useState(false);
  const [modeError, setModeError] = useState("");
  const generation = useRef(0);
  const checkingRef = useRef(false);
  const mode = settings?.routingMode ?? "rule";
  useEffect(() => {
    generation.current++;
    checkingRef.current = false;
    setChecking(false);
    setResults({});
    return () => {
      generation.current++;
    };
  }, [
    connected,
    mode,
    settings?.selected,
    JSON.stringify(settings?.groups),
    settings?.defaultRoute,
  ]);
  async function checkSites() {
    if (checkingRef.current || !connected) return;
    checkingRef.current = true;
    const current = ++generation.current;
    setChecking(true);
    setResults({});
    await Promise.all(
      sites.map(async ([id]) => {
        let result: SiteResult;
        try {
          result = await request<SiteResult>("site_check", { id });
        } catch (e) {
          result = { ok: false, ms: null, status: null, error: String(e) };
        }
        if (current === generation.current)
          setResults((old) => ({ ...old, [id]: result }));
      }),
    );
    if (current === generation.current) {
      checkingRef.current = false;
      setChecking(false);
    }
  }
  const latest = samples.at(-1);
  const maximum = Math.max(1250, ...samples.flatMap((p) => [p.up, p.down]));
  const now = latest?.time ?? Date.now();
  const line = (key: "up" | "down") =>
    samples
      .map(
        (p) =>
          `${((p.time - now + 600000) / 600000) * 1000},${150 - (p[key] / maximum) * 140}`,
      )
      .join(" ");
  const value = (n: number | null) =>
    connected && !error && n !== null ? megabytes(n) : "—";
  return (
    <div className="dashboard-tools">
      <section className="dashboard-panel" aria-labelledby="mode-title">
        <header>
          <h2 id="mode-title">
            <SlidersHorizontal size={19} />
            Режим работы
          </h2>
        </header>
        <div
          className="routing-modes"
          role="group"
          aria-label="Режим маршрутизации"
        >
          {modes.map(([id, label]) => (
            <button
              key={id}
              aria-pressed={mode === id}
              className={mode === id ? "primary" : ""}
              disabled={!settings || busy}
              onClick={async () => {
                if (!settings || mode === id) return;
                setModeError("");
                try {
                  await save({ ...settings, routingMode: id });
                } catch (e) {
                  setModeError(String(e));
                }
              }}
            >
              {label}
            </button>
          ))}
        </div>
        <p className="mode-description">
          {modes.find(([id]) => id === mode)?.[2]}
        </p>
        <p className="dashboard-note">
          {connected
            ? "Режим применяется к новым соединениям."
            : "Выбранный режим будет применён при подключении."}
        </p>
        {modeError && (
          <p role="alert" className="site-failed">
            {modeError}
          </p>
        )}
      </section>
      <section className="dashboard-panel" aria-labelledby="traffic-title">
        <header>
          <h2 id="traffic-title">
            <Activity size={19} />
            Статистика трафика
          </h2>
          <span>Последние 10 минут</span>
        </header>
        <div className="traffic-legend">
          <span className="traffic-upload">
            <ArrowUp size={14} />
            Отправка
          </span>
          <span className="traffic-download">
            <ArrowDown size={14} />
            Скачивание
          </span>
          <span>До {kilobytesPerSecond(maximum)}</span>
        </div>
        <div className="traffic-chart">
          <svg
            viewBox="0 0 1000 160"
            preserveAspectRatio="none"
            role="img"
            aria-label="Скорость отправки и скачивания за последние 10 минут"
          >
            {[10, 80, 150].map((y) => (
              <line
                key={y}
                x1="0"
                x2="1000"
                y1={y}
                y2={y}
                className="chart-grid"
              />
            ))}
            {connected && !error && (
              <>
                <polyline points={line("up")} className="traffic-upload" />
                <polyline points={line("down")} className="traffic-download" />
              </>
            )}
          </svg>
          {(!connected || error || samples.length < 2) && (
            <p>
              {!connected
                ? "Подключите Atlas для сбора статистики"
                : error
                  ? "Не удалось обновить статистику. Повторяем…"
                  : "Собираем данные трафика…"}
            </p>
          )}
        </div>
        <div className="chart-times">
          <span>10 мин назад</span>
          <span>Сейчас</span>
        </div>
        <dl className="traffic-metrics">
          {[
            [
              "Скорость отправки",
              latest && connected && !error
                ? kilobytesPerSecond(latest.up)
                : "—",
            ],
            [
              "Скорость скачивания",
              latest && connected && !error
                ? kilobytesPerSecond(latest.down)
                : "—",
            ],
            ["Активные соединения", connected && !error ? connections : "—"],
            ["Отправлено за сессию", value(traffic.up)],
            ["Скачано за сессию", value(traffic.down)],
            ["Память ядра", value(memory)],
          ].map(([label, val]) => (
            <div key={label}>
              <dt>{label}</dt>
              <dd>{val}</dd>
            </div>
          ))}
        </dl>
      </section>
      <section className="dashboard-panel" aria-labelledby="sites-title">
        <header>
          <h2 id="sites-title">
            <Wifi size={19} />
            Доступность сайтов
          </h2>
          <button
            onClick={() => void checkSites()}
            disabled={!connected || checking}
            aria-label="Проверить все сайты"
          >
            <RefreshCw size={15} />
            {checking ? "Проверяем…" : "Проверить"}
          </button>
        </header>
        <div className="site-grid">
          {sites.map(([id, name]) => {
            const result = results[id];
            return (
              <div className="site-result" key={id}>
                <Globe2 size={23} />
                <strong>{name}</strong>
                <span
                  className={
                    result?.ok ? "site-ok" : result ? "site-failed" : ""
                  }
                  title={result?.error}
                >
                  {result
                    ? result.ok
                      ? `HTTP-ответ: ${result.ms} мс`
                      : result.status
                        ? `HTTP ${result.status}`
                        : "Нет ответа"
                    : checking
                      ? "Проверяем…"
                      : "Не проверен"}
                </span>
              </div>
            );
          })}
        </div>
        <p className="dashboard-note">
          {connected
            ? "Время HTTP-ответа, не сетевой пинг: включает установку соединения, TLS, перенаправления и ожидание ответа сайта. Ответ 403/429 может означать защиту от автоматических запросов."
            : "Подключите Atlas, чтобы проверить доступность сервисов."}
        </p>
      </section>
    </div>
  );
}
