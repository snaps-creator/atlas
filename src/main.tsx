import React, { useEffect, useState, useCallback, useRef } from "react";
import { createRoot } from "react-dom/client";
import { check } from "@tauri-apps/plugin-updater";
import { getVersion } from "@tauri-apps/api/app";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { startUpdatePolling } from "./updatePolling";
import {
  Activity,
  ArrowDown,
  ArrowUp,
  ArrowUpRight,
  Check,
  ChevronRight,
  Compass,
  Download,
  Globe2,
  GripVertical,
  LayoutDashboard,
  ListFilter,
  LoaderCircle,
  Plus,
  Power,
  RefreshCw,
  Search,
  Server,
  Settings2,
  Shield,
  SlidersHorizontal,
  Star,
  Stethoscope,
  Trash2,
  Wifi,
  X,
  FileText,
  Copy,
} from "lucide-react";
import { request, download, bytes, native } from "./api";
import { RouteSelect, routeName } from "./RuleEditor";
import type { Snapshot, Settings, Group, Connection } from "./types";
import "./style.css";
import "./arcade.css";
import { RulesPanel } from "./RulesPanel";
import { RuleChecks } from "./RuleChecks";
import { DashboardTools } from "./DashboardTools";
import { trafficRate, type TrafficSample } from "./traffic";
import { ActiveServer } from "./ActiveServer";
import { ConnectionRules, connectionRoute } from "./ConnectionRules";
import "flag-icons/css/flag-icons.min.css";
import { boundedLatency, latencyLabel, testPool, LatencyEpoch, type Latency } from "./latency";
type AvailableUpdate = NonNullable<Awaited<ReturnType<typeof check>>>;
type UpdateStatus = "idle" | "downloading" | "installing" | "error";
import { type ProtectionStatus, unavailableProtection, protectionLabel } from "./protection";
const autoTestIntervals = [30, 60, 120, 300, 600, 900, 1800, 3600];
function intervalLabel(seconds: number) {
  return seconds < 60 ? `${seconds} сек.` : `${seconds / 60} мин.`;
}
const nav = [
  ["Dashboard", LayoutDashboard],
  ["Servers", Server],
  ["Rules", SlidersHorizontal],
  ["Connections", Activity],
  ["DNS", Globe2],
  ["Subscriptions", Download],
  ["Diagnostics", Stethoscope],
  ["Logs", FileText],
  ["Settings", Settings2],
  ["Updates", RefreshCw],
] as const;
const pageNames: Record<string, string> = {
  Dashboard: "Главная",
  Servers: "Серверы",
  Rules: "Правила",
  Connections: "Подключения",
  DNS: "DNS",
  Subscriptions: "Подписки",
  Diagnostics: "Диагностика",
  Logs: "Журнал",
  Settings: "Настройки",
  Updates: "Обновления",
};
function App() {
  const [versionLabel, setVersionLabel] = useState("");
  const [appVersion, setAppVersion] = useState("");
  useEffect(() => {
    if (!native) return;
    void getVersion()
      .then(async (version) => {
        setAppVersion(version);
        const beta = /^1\.0\.0-beta\.(\d+(?:\.\d+)*)$/.exec(version);
        const label = beta ? `Beta ${beta[1]}` : version;
        setVersionLabel(label);
        await getCurrentWindow().setTitle("atlas");
      })
      .catch((reason) => console.warn("Не удалось прочитать версию Atlas", reason));
  }, []);
  const [page, setPage] = useState("Dashboard");
  const [data, setData] = useState<Snapshot | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [reportBusy, setReportBusy] = useState(false);
  const [reportSaved, setReportSaved] = useState(false);
  const [query, setQuery] = useState("");
  const [adding, setAdding] = useState(false);
  const [url, setUrl] = useState("");
  const [name, setName] = useState("");
  const [latencies, setLatencies] = useState<Record<string, Latency>>({});
  const [testingAll, setTestingAll] = useState(false);
  const latencyEpoch = useRef(new LatencyEpoch());
  const [connections, setConnections] = useState<Connection[]>([]);
  const [traffic, setTraffic] = useState({ up: 0, down: 0 });
  const [samples, setSamples] = useState<TrafficSample[]>([]);
  const [memory, setMemory] = useState<number | null>(null);
  const [trafficError, setTrafficError] = useState("");
  const [inspection, setInspection] = useState<Connection | null>(null);
  const [checks, setChecks] = useState<
    { name: string; ok: boolean | null; detail: string }[]
  >([]);
  const [publicIp, setPublicIp] = useState("");
  const [importText, setImportText] = useState("");
  const [preview, setPreview] = useState<{
    groups: Group[];
    warnings: string[];
  } | null>(null);
  const [dnsText, setDnsText] = useState("");
  const [onlyFavorites, setOnlyFavorites] = useState(false);
  const [logLevel, setLogLevel] = useState("ALL");
  const [sortLatency, setSortLatency] = useState(false);
  const protectionCheckRunning = useRef(false);
  const [availableUpdate, setAvailableUpdate] =
    useState<AvailableUpdate | null>(null);
  const [updateStatus, setUpdateStatus] = useState<UpdateStatus>("idle");
  const [updateProgress, setUpdateProgress] = useState(0);
  const [updateError, setUpdateError] = useState("");
  const checkUpdatesNow = useRef<(() => Promise<void>) | null>(null);
  const [updateCheckStatus, setUpdateCheckStatus] = useState<"idle" | "checking" | "current" | "available" | "error">("idle");
  const [updateCheckError, setUpdateCheckError] = useState("");
  const [updateCheckedAt, setUpdateCheckedAt] = useState<number | null>(null);
  const [protectionChecking, setProtectionChecking] = useState(false);
  const [protection, setProtection] = useState<ProtectionStatus>({
    secure: false,
    detail: "Atlas не подключён.",
    checkedAt: 0,
  });
  const [activeServer, setActiveServer] = useState<string | null>(null);
  const refreshPending = useRef(false);
  const refresh = useCallback(async () => {
    if (refreshPending.current) return;
    refreshPending.current = true;
    try {
      setData(await request<Snapshot>("snapshot"));
    } catch (e) {
      if (!String(e).includes("Atlas занят")) setError(String(e));
    } finally {
      refreshPending.current = false;
    }
  }, []);
  useEffect(() => {
    void refresh();
    const timer = setInterval(() => {
      void refresh();
    }, 2500);
    return () => clearInterval(timer);
  }, [refresh]);
  useEffect(() => {
    if (!native) return;
    const polling = startUpdatePolling(
      () => check({ timeout: 15000 }),
      (update) => {
        setAvailableUpdate(update);
        setUpdateCheckStatus("available");
        setUpdateCheckedAt(Date.now());
      },
      (reason) => {
        setUpdateCheckStatus("error");
        setUpdateCheckError(String(reason));
        setUpdateCheckedAt(Date.now());
      },
      {
        onStart: () => { setUpdateCheckStatus("checking"); setUpdateCheckError(""); },
        onCurrent: () => { setUpdateCheckStatus("current"); setUpdateCheckedAt(Date.now()); },
      },
    );
    checkUpdatesNow.current = polling.checkNow;
    return () => { polling(); checkUpdatesNow.current = null; };
  }, []);
  useEffect(() => {
    if (data) document.documentElement.dataset.theme = data.settings.theme;
  }, [data?.settings.theme]);
  const protectionKey = data
    ? [
        data.running,
        data.status,
        data.settings.defaultRoute,
        data.settings.routingMode,
        data.settings.selected,
        data.settings.dns.servers.join("|"),
        data.settings.groups
          .filter((group) => group.enabled)
          .map((group) => `${group.id}:${group.route}:${group.rules.length}`)
          .join("|"),
      ].join(";")
    : "disconnected";
  useEffect(() => {
    let alive = true;
    let timer: ReturnType<typeof setTimeout> | undefined;
    setProtectionChecking(false);
    setProtection({ secure: null, detail: "Проверяем текущее соединение…", checkedAt: 0 });
    const schedule = (ms: number) => { if (alive) timer = setTimeout(inspect, ms); };
    const inspect = async () => {
      if (!alive) return;
      if (!data?.running || data.status !== "Connected") {
        setProtection({ secure: false, detail: "Atlas не подключён.", checkedAt: Math.floor(Date.now() / 1000) });
        return;
      }
      if (protectionCheckRunning.current) { schedule(1000); return; }
      protectionCheckRunning.current = true;
      setProtectionChecking(true);
      let retry = 30000;
      try {
        const result = await request<ProtectionStatus>("protection_status");
        if (alive) setProtection(previous => ({ ...result, lastConfirmedAt: result.secure === true ? result.checkedAt : previous.lastConfirmedAt }));
      } catch (reason) {
        retry = String(reason).includes("занят") ? 3000 : 10000;
        if (alive) setProtection(previous => unavailableProtection(previous, reason));
      } finally {
        protectionCheckRunning.current = false;
        if (alive) setProtectionChecking(false);
        schedule(retry);
      }
    };
    void inspect();
    return () => { alive = false; if (timer !== undefined) clearTimeout(timer); };
  }, [protectionKey]);
  useEffect(() => {
    setQuery("");
    if (page === "DNS" && data)
      setDnsText(data.settings.dns.servers.join("\n"));
  }, [page]);
  useEffect(() => {
    setSamples([]);
    setTraffic({ up: 0, down: 0 });
    setMemory(null);
    setTrafficError("");
    if (!data?.running) return;
    let alive = true;
    let pending = false;
    let previous: { time: number; up: number; down: number } | null = null;
    const poll = async () => {
      if (pending) return;
      pending = true;
      try {
        const r = await request<{
          connections: Connection[];
          uploadTotal: number;
          downloadTotal: number;
          memory?: number;
        }>("connections");
        if (alive) {
          setConnections(r.connections ?? []);
          const time = Date.now();
          const sample = trafficRate(previous, { time, up: r.uploadTotal, down: r.downloadTotal });
          if (sample) {
            setSamples(old => [...old.filter(point => point.time >= time - 600000), sample]);
          }
          previous = { time, up: r.uploadTotal, down: r.downloadTotal };
          setTraffic({ up: r.uploadTotal, down: r.downloadTotal });
          setMemory(r.memory ?? null);
          setTrafficError("");
        }
      } catch (e) {
        if (alive && !String(e).includes("занят")) setTrafficError(String(e));
      } finally {
        pending = false;
      }
    };
    void poll();
    const t = setInterval(poll, 2500);
    return () => {
      alive = false;
      clearInterval(t);
    };
  }, [data?.running]);
  async function act(action: string, payload?: unknown) {
    setBusy(true);
    setError("");
    try {
      const r = await request<Snapshot>(action, payload);
      if (r?.settings) setData(r);
      return r;
    } catch (e) {
      setError(String(e));
      throw e;
    } finally {
      setBusy(false);
    }
  }
  async function save(s: Settings) {
    await act("save", s);
  }
  function run(f: () => Promise<unknown>) {
    void f().catch(() => {});
  }
  async function installUpdate() {
    if (!availableUpdate || updateStatus === "downloading" || updateStatus === "installing") return;
    setUpdateStatus("downloading");
    setUpdateProgress(0);
    setUpdateError("");
    let downloaded = 0;
    let total = 0;
    try {
      await availableUpdate.download((event) => {
        if (event.event === "Started") {
          total = event.data.contentLength ?? 0;
        } else if (event.event === "Progress") {
          downloaded += event.data.chunkLength;
          if (total > 0)
            setUpdateProgress(Math.min(100, Math.round((downloaded / total) * 100)));
        } else if (event.event === "Finished") {
          setUpdateProgress(100);
          setUpdateStatus("installing");
        }
      });
      setUpdateStatus("installing");
      if (data?.running || data?.guardActive) await request("disconnect");
      await availableUpdate.install({ restartAfterInstall: true });
    } catch (reason) {
      setUpdateStatus("error");
      setUpdateError(`Не удалось установить обновление: ${String(reason)}`);
    }
  }
  const s = data?.settings;
  const servers = s?.subscriptions.flatMap((v) => v.servers) ?? [];
  const connected = data?.running ?? false;
  const connecting = data?.status === "Connecting";
  const header = (
    eyebrow: string,
    title: string,
    description: string,
    action?: React.ReactNode,
  ) => (
    <div className="page-head">
      <div>
        <h1>{pageNames[page] ?? title}</h1>
      </div>
      {action}
    </div>
  );
  const search = (
    <div className="search">
      <Search size={16} />
      <input
        aria-label="Поиск"
        placeholder="Поиск"
        value={query}
        onChange={(e) => setQuery(e.target.value)}
      />
    </div>
  );
  const empty = (title: string, text: string, button?: React.ReactNode) => (
    <div className="empty">
      <Compass size={38} strokeWidth={1} />
      <h2>{title}</h2>
      <p>{text}</p>
      {button}
    </div>
  );
  const addButton = (
    <button className="primary" onClick={() => setAdding(true)}>
      <Plus size={16} />
      Добавить подписку
    </button>
  );
  const latencyContext = JSON.stringify([data?.revision, data?.running, data?.settings.selected,
    data?.settings.subscriptions.map(sub => [sub.id, sub.updatedAt, sub.servers])]);
  useEffect(() => {
    if (latencyEpoch.current.update(latencyContext)) setLatencies({});
    return () => { latencyEpoch.current.update("unmounted"); };
  }, [latencyContext]);
  async function test(name: string) {
    const token = latencyEpoch.current.begin(latencyContext, name);
    if (!token) return;
    setLatencies((l) => ({
      ...l,
      [name]: { status: "testing", delay: null, attempts: 0 },
    }));
    try {
      const r = await boundedLatency(request<Latency>("latency", { name }));
      if (latencyEpoch.current.current(name, token)) setLatencies((l) => ({ ...l, [name]: r }));
    } catch (e) {
      if (latencyEpoch.current.current(name, token)) setLatencies((l) => ({
        ...l,
        [name]: { status: "error", delay: null, attempts: 0, error: String(e) },
      }));
    } finally {
      latencyEpoch.current.finish(name, token);
    }
  }
  return (
    <div className="app">
      <aside>
        <a
          className="brand"
          href="#"
          onClick={(e) => {
            e.preventDefault();
            setPage("Dashboard");
          }}
        >
          <span className="brand-icon">
            <Compass size={25} />
          </span>
          atlas<span className="brand-dot">.</span>
        </a>
        <div className="workspace">
          <ActiveServer connected={connected} onChange={setActiveServer} />
          <button
            role="switch"
            aria-label="Подключение VPN"
            aria-checked={connected}
            title={data?.error ?? undefined}
            className={`toggle ${connected ? "on" : ""}`}
            disabled={
              busy ||
              !data ||
              (!connected && !data?.guardActive && !servers.length)
            }
            onClick={() =>
              run(() =>
                act(connected || data?.guardActive ? "disconnect" : "connect"),
              )
            }
          >
            <span />
          </button>
        </div>
        <div className="nav-label">УПРАВЛЕНИЕ</div>
        <nav>
          {nav.map(([p, Icon]) => (
            <button
              key={p}
              className={page === p ? "active" : ""}
              onClick={() => setPage(p)}
            >
              <Icon size={18} strokeWidth={1.7} />
              <span>{pageNames[p]}</span>
              {p === "Subscriptions" && !!s?.subscriptions.length && (
                <small>{s.subscriptions.length}</small>
              )}
            </button>
          ))}
        </nav>
        <div className="sidebar-bottom">
          <div
            className={`privacy ${protection.secure === null ? "unknown" : protection.secure ? "protected" : "unprotected"}`}
            title={protection.detail}
            aria-live="polite"
          >
            <Shield size={15} />
            <span>
              {protectionLabel(protection)}
              {protectionChecking && data?.running && data.status === "Connected" && (
                <RefreshCw
                  size={12}
                  className="spin protection-check-spinner"
                  aria-label="Проверка защиты"
                />
              )}
              <small>{protection.detail}</small>
              {protection.secure === null && protection.lastConfirmedAt && (
                <small>Последнее подтверждение: {new Date(protection.lastConfirmedAt * 1000).toLocaleString("ru-RU")}</small>
              )}
            </span>
          </div>
          <div className="version">
            <span>atlas by TLQ</span>
            {availableUpdate ? (
              <button
                className="sidebar-update"
                title={updateError || `Обновить до Atlas ${availableUpdate.version}`}
                disabled={updateStatus === "downloading" || updateStatus === "installing"}
                onClick={() => { setPage("Updates"); void installUpdate(); }}
              >
                {updateStatus === "downloading" ? `${updateProgress}%` : updateStatus === "installing" ? "Установка…" : "Обновить"}
              </button>
            ) : <span>{versionLabel}</span>}
          </div>
        </div>
      </aside>
      <div className="content">
        <header className="topbar">
          <span>
            Атлас <ChevronRight size={13} /> <strong>{pageNames[page]}</strong>
          </span>
          <span className="top-status">
            <i className={connected ? "dot online" : "dot"} />
            {connected ? "Подключено" : "Не подключено"}
          </span>
        </header>
        <main>
          {page === "Updates" && (
            <>
              <div className="page-heading"><h1>Обновления</h1></div>
              <section className="settings-section update-settings">
                <h2>Текущая версия</h2>
                <p>{versionLabel || "Версия недоступна"}{appVersion ? ` · ${appVersion}` : ""}</p>
                <p>Автоматическая проверка при запуске и каждые 6 часов.</p>
                <button
                  disabled={!native || updateCheckStatus === "checking" || !!availableUpdate}
                  onClick={() => void checkUpdatesNow.current?.()}
                >
                  <RefreshCw size={16} className={updateCheckStatus === "checking" ? "spin" : undefined} />
                  {updateCheckStatus === "checking" ? "Проверяем…" : "Проверить обновления"}
                </button>
                <p role="status" aria-live="polite">
                  {updateCheckStatus === "checking" ? "Проверяем GitHub…"
                    : updateCheckStatus === "current" ? "Установлена последняя версия."
                    : updateCheckStatus === "available" ? `Доступна новая версия: ${availableUpdate?.version}.`
                    : updateCheckStatus === "error" ? "Не удалось проверить обновления. Проверьте подключение к GitHub и повторите попытку."
                    : "Проверка ещё не выполнена."}
                </p>
                {updateCheckError && <details className="update-error-details"><summary>Подробности ошибки</summary><p>{updateCheckError}</p></details>}
                <p>Последняя проверка: {updateCheckedAt ? new Date(updateCheckedAt).toLocaleString("ru-RU") : "ещё не выполнялась"}</p>
              </section>
            </>
          )}
          {page === "Updates" && availableUpdate && (
            <section className="update-banner" aria-live="polite">
              <div className="update-copy">
                <Download size={18} />
                <div>
                  <strong>Доступно обновление Atlas {availableUpdate.version}</strong>
                  <p>
                    {updateStatus === "downloading"
                      ? `Загрузка: ${updateProgress}%`
                      : updateStatus === "installing"
                        ? "Установка обновления…"
                        : updateError || "Можно скачать и установить новую версию."}
                  </p>
                  {(updateStatus === "downloading" || updateStatus === "installing") && (
                    <div className="update-progress" aria-label={`Загрузка ${updateProgress}%`}>
                      <span style={{ width: `${updateProgress}%` }} />
                    </div>
                  )}
                </div>
              </div>
              <button
                className="primary"
                disabled={updateStatus === "downloading" || updateStatus === "installing"}
                onClick={() => void installUpdate()}
              >
                {updateStatus === "downloading" || updateStatus === "installing" ? (
                  <LoaderCircle className="spin" size={16} />
                ) : (
                  <Download size={16} />
                )}
                {updateStatus === "error" ? "Повторить" : "Обновить"}
              </button>
            </section>
          )}
          <RuleChecks
            key={s?.routingMode ?? "rule"}
            settings={s}
            connected={connected}
            visible={page === "Rules"}
            refresh={refresh}
          />
          {data?.guardActive && !connected && (
            <div className="alert" role="alert">
              <span>VPN недоступен. Защита блокирует сеть.</span>
              <button
                disabled={busy}
                onClick={() => run(() => act("disconnect"))}
              >
                Отключить защиту
              </button>
            </div>
          )}
          {error && (
            <div role="alert" className="alert">
              <span>{error}</span>
              <button aria-label="Закрыть ошибку" onClick={() => setError("")}>
                <X size={16} />
              </button>
            </div>
          )}
          {!data ? (
            <>
              {header(
                "ATLAS FOR WINDOWS",
                "Всё начинается с подключения.",
                "Ваше пространство для безопасного и управляемого интернета.",
              )}
              {empty(
                native ? "Загрузка приложения…" : "Откройте приложение Atlas",
                native
                  ? "Подключаемся к локальным службам."
                  : "Управление VPN доступно в установленном Windows-приложении.",
              )}
            </>
          ) : (
            <>
              {page === "Dashboard" && (
                <>
                  {header(
                    "YOUR NETWORK, YOUR RULES",
                    "Интернет. На ваших условиях.",
                    "Одно подключение. Полный контроль над каждым маршрутом.",
                    <button onClick={() => setPage("Diagnostics")}>
                      <Stethoscope size={16} />
                      Диагностика
                    </button>,
                  )}
                  <section
                    className={`connection-hero ${connected ? "is-connected" : ""}`}
                  >
                    <div className="hero-top">
                      <span className="badge">
                        <i className={connected ? "dot online" : "dot"} />
                        {connecting ? "ПОДКЛЮЧЕНИЕ…" : data.status === "Disconnecting" ? "ОТКЛЮЧЕНИЕ…" : connected
                          ? "ТУННЕЛЬ ЗАПУЩЕН"
                          : data.status === "Error"
                            ? "ОШИБКА ПОДКЛЮЧЕНИЯ"
                            : "ГОТОВ К ПОДКЛЮЧЕНИЮ"}
                      </span>
                      <span className="muted">
                        {s?.mode === "tun"
                          ? "Вся система · TUN"
                          : "Системный прокси"}{" "}
                        <Shield size={14} />
                      </span>
                    </div>
                    <div className="hero-main">
                      <button
                        aria-label={
                          connecting ? "Отменить подключение" : connected ? "Отключить VPN" : "Подключить VPN"
                        }
                        className="power"
                        disabled={(busy && !connecting) || !servers.length}
                        onClick={() =>
                          run(() =>
                            act(
                              connected || connecting || data?.guardActive
                                ? "disconnect"
                                : "connect",
                            ),
                          )
                        }
                      >
                        {busy ? (
                          <LoaderCircle className="spin" size={32} />
                        ) : (
                          <Power size={32} strokeWidth={1.4} />
                        )}
                      </button>
                      <div>
                        <h2>{connecting ? "Подключение…" : data.status === "Disconnecting" ? "Отключение…" : connected ? "Туннель запущен" : "Отключено"}</h2>
                        <p>
                          {connected
                            ? protection.detail
                            : "Выберите сервер и подключитесь"}
                        </p>
                      </div>
                      <button
                        className="primary connect-btn"
                        disabled={(busy && !connecting) || !servers.length}
                        onClick={() =>
                          run(() =>
                            act(
                              connected || connecting || data?.guardActive
                                ? "disconnect"
                                : "connect",
                            ),
                          )
                        }
                      >
                        {connecting ? "Отменить" : connected ? "Отключить" : "Подключить"}
                        <ArrowUpRight size={17} />
                      </button>
                    </div>
                    <div className="hero-details">
                      <div>
                        <small>ТЕКУЩИЙ СЕРВЕР</small>
                        <button
                          className="text-button"
                          onClick={() => setPage("Servers")}
                        >
                          <Globe2 size={16} />
                          {s?.selected === "AUTO"
                            ? "Автоматический выбор"
                            : s?.selected}
                          <ChevronRight size={14} />
                        </button>
                      </div>
                      <div>
                        <small>ПУБЛИЧНЫЙ IP</small>
                        <button
                          className="text-button"
                          disabled={!connected}
                          onClick={() =>
                            run(async () => {
                              try {
                                setPublicIp(
                                  (await request<{ ip: string }>("public_ip"))
                                    .ip,
                                );
                              } catch (e) {
                                setError(String(e));
                              }
                            })
                          }
                        >
                          {publicIp || "Проверить IP"}
                          <ArrowUpRight size={14} />
                        </button>
                      </div>
                      <div>
                        <small>ВРЕМЯ СЕССИИ</small>
                        <span>
                          {connected
                            ? `${Math.floor(data.duration / 3600)
                                .toString()
                                .padStart(
                                  2,
                                  "0",
                                )}:${Math.floor(data.duration / 60) % 60 < 10 ? "0" : ""}${Math.floor(data.duration / 60) % 60}:${(data.duration % 60).toString().padStart(2, "0")}`
                            : "—"}
                        </span>
                      </div>
                    </div>
                  </section>
                  <DashboardTools connected={connected} settings={s} busy={busy} save={save} samples={samples} traffic={traffic} memory={memory} connections={connections.length} error={trafficError} />
                  <section className="section">
                    <div className="default-route">
                      <span>
                        <Globe2 size={15} />
                        Остальной трафик
                      </span>
                      <span>Если ни одно правило не совпало</span>
                      {s && (
                        <RouteSelect
                          value={s.defaultRoute}
                          onChange={(r) =>
                            run(() => save({ ...s, defaultRoute: r }))
                          }
                        />
                      )}
                    </div>
                  </section>
                  {!servers.length && (
                    <div className="onboarding">
                      <div>
                        <span className="eyebrow">ПЕРВЫЙ ШАГ</span>
                        <h2>Добавьте вашу подписку.</h2>
                        <p>
                          Вставьте HTTPS-ссылку от VPN-провайдера. Atlas
                          загрузит и проверит серверы.
                        </p>
                      </div>
                      {addButton}
                    </div>
                  )}
                </>
              )}
              {page === "Servers" && (
                <>
                  {header(
                    "GLOBAL NETWORK",
                    "Найдите свой маршрут.",
                    "Серверы из ваших подписок. Доступность проверяется через Mihomo.",
                    <button
                      disabled={!connected || testingAll}
                      onClick={() =>
                        run(async () => {
                          setTestingAll(true);
                          try {
                            await testPool(servers, (n) => test(n.name));
                          } finally {
                            setTestingAll(false);
                          }
                        })
                      }
                    >
                      <Activity size={16} />
                      Проверить все
                    </button>,
                  )}
                  <div className="toolbar">
                    {search}
                    <button
                      className={onlyFavorites ? "selected" : ""}
                      onClick={() => setOnlyFavorites(!onlyFavorites)}
                    >
                      <Star size={16} />
                      Избранное
                    </button>
                    <button onClick={() => setSortLatency(!sortLatency)}>
                      {sortLatency ? "Задержка ↑" : "По порядку"}
                    </button>
                  </div>
                  <div className="auto-options">
                    {["AUTO", "FAILOVER"].map((n) => (
                      <button
                        key={n}
                        className={s?.selected === n ? "selected" : ""}
                        onClick={() =>
                          s && run(() => save({ ...s, selected: n }))
                        }
                      >
                        <Wifi size={20} />
                        <span>
                          <strong>
                            {n === "AUTO"
                              ? "Автоматический выбор"
                              : "Резервное переключение"}
                          </strong>
                          <small>
                            {n === "AUTO"
                              ? `Тест каждые ${intervalLabel(s?.autoTestIntervalSeconds ?? 300)} · порог 50 мс`
                              : `Первый доступный сервер · проверка каждые ${intervalLabel(s?.autoTestIntervalSeconds ?? 300)}`}
                          </small>
                        </span>
                        {s?.selected === n && <Check size={17} />}
                      </button>
                    ))}
                  </div>
                  {servers.length ? (
                    <div className="list">
                      {servers
                        .filter(
                          (n) =>
                            n.name
                              .toLowerCase()
                              .includes(query.toLowerCase()) &&
                            (!onlyFavorites || s?.favorites.includes(n.name)),
                        )
                        .sort((a, b) =>
                          sortLatency
                            ? (latencies[a.name]?.delay ?? Infinity) -
                              (latencies[b.name]?.delay ?? Infinity)
                            : 0,
                        )
                        .map((n) => (
                          <div
                            className={`server-row ${
                              connected && activeServer === n.name
                                ? "connected-server"
                                : ""
                            }`}
                            key={n.name}
                          >
                            <button
                              aria-label="В избранное"
                              className="icon-button"
                              onClick={() =>
                                s &&
                                run(() =>
                                  save({
                                    ...s,
                                    favorites: s.favorites.includes(n.name)
                                      ? s.favorites.filter((f) => f !== n.name)
                                      : [...s.favorites, n.name],
                                  }),
                                )
                              }
                            >
                              <Star
                                size={17}
                                fill={
                                  s?.favorites.includes(n.name)
                                    ? "currentColor"
                                    : "none"
                                }
                              />
                            </button>
                            {n.country ? (
                              <span
                                className={`fi fi-${n.country}`}
                                role="img"
                                aria-label={n.country.toUpperCase()}
                              />
                            ) : (
                              <Globe2 size={22} />
                            )}
                            <div className="grow">
                              <strong>{n.name}</strong>
                              <small>{n.type.toUpperCase()}</small>
                            </div>
                            <button
                              disabled={
                                !connected ||
                                testingAll ||
                                latencies[n.name]?.status === "testing"
                              }
                              title={
                                latencies[n.name]?.error ?? "Проверить сервер"
                              }
                              onClick={() => run(() => test(n.name))}
                            >
                              {latencyLabel(latencies[n.name])}
                            </button>
                            <button
                              className={
                                s?.selected === n.name ? "selected" : ""
                              }
                              onClick={() =>
                                s && run(() => save({ ...s, selected: n.name }))
                              }
                            >
                              {s?.selected === n.name ? (
                                <>
                                  <Check size={14} />
                                  Выбран
                                </>
                              ) : (
                                "Выбрать"
                              )}
                            </button>
                          </div>
                        ))}
                    </div>
                  ) : (
                    empty(
                      "Пока нет серверов",
                      "Добавьте подписку, чтобы загрузить доступные серверы.",
                      addButton,
                    )
                  )}
                  <p className="footnote">
                    Для измерения задержки запустите подключение. Показатели не
                    рассчитываются и не подставляются заранее.
                  </p>
                </>
              )}
              {page === "Rules" && s && (
                <RulesPanel settings={s} save={save} refresh={refresh} />
              )}
              {page === "Subscriptions" && (
                <>
                  {header(
                    "YOUR PROVIDERS",
                    "Одна ссылка. Вся сеть.",
                    "Секретные URL хранятся в Windows Credential Manager.",
                    addButton,
                  )}
                  {s?.subscriptions.length ? (
                    <div className="list">
                      {s.subscriptions.map((sub) => (
                        <div className="subscription" key={sub.id}>
                          <div className="section-head">
                            <div>
                              <h2>{sub.name}</h2>
                              <p>{sub.maskedUrl}</p>
                            </div>
                            <div className="actions">
                              <button
                                disabled={busy}
                                onClick={() =>
                                  run(() =>
                                    act("subscription_refresh", { id: sub.id }),
                                  )
                                }
                              >
                                <RefreshCw size={15} />
                                Обновить
                              </button>
                              <button
                                aria-label="Удалить подписку"
                                disabled={busy || connected}
                                onClick={() =>
                                  run(() =>
                                    act("subscription_delete", { id: sub.id }),
                                  )
                                }
                              >
                                <Trash2 size={16} />
                              </button>
                            </div>
                          </div>
                          <div className="subscription-meta">
                            <span>{sub.servers.length} серверов</span>
                            <span>
                              Обновлена{" "}
                              {new Date(sub.updatedAt * 1000).toLocaleString(
                                "ru",
                              )}
                            </span>
                          </div>
                          {sub.error && <p className="error">{sub.error}</p>}
                        </div>
                      ))}
                    </div>
                  ) : (
                    empty(
                      "Подключите вашего провайдера",
                      "Поддерживаются Clash/Mihomo YAML, Base64 и распространённые proxy URI.",
                      addButton,
                    )
                  )}
                </>
              )}
              {page === "Connections" && (
                <>
                  {header(
                    "LIVE ACTIVITY",
                    "У каждого соединения есть история.",
                    "Нажмите на соединение, чтобы увидеть сработавшее правило.",
                  )}
                  {search}
                  {!connected ? (
                    empty(
                      "Соединение не запущено",
                      "Подключитесь, чтобы просматривать текущий трафик.",
                    )
                  ) : connections.length ? (
                    <div className="table-wrap">
                      <table>
                        <thead>
                          <tr>
                            <th>Приложение</th>
                            <th>Домен / IP</th>
                            <th>Протокол</th>
                            <th>IP</th>
                            <th>↓ / ↑</th>
                            <th>Правило</th>
                            <th>Маршрут</th>
                            <th>Сервер</th>
                          </tr>
                        </thead>
                        <tbody>
                          {connections
                            .filter((c) =>
                              JSON.stringify(c.metadata)
                                .toLowerCase()
                                .includes(query.toLowerCase()),
                            )
                            .map((c) => (
                              <tr
                                tabIndex={0}
                                key={c.id}
                                onClick={() => setInspection(c)}
                                onKeyDown={(e) =>
                                  e.key === "Enter" && setInspection(c)
                                }
                              >
                                <td>{c.metadata.process || "—"}</td>
                                <td>
                                  <strong>{c.metadata.host || "—"}</strong>
                                  <small>{c.metadata.destinationIP}</small>
                                </td>
                                <td>{c.metadata.network.toUpperCase()}</td>
                                <td>
                                  {c.metadata.destinationIP
                                    ? c.metadata.destinationIP.includes(":")
                                      ? "IPv6"
                                      : "IPv4"
                                    : "—"}
                                </td>
                                <td>
                                  {bytes(c.download)} / {bytes(c.upload)}
                                </td>
                                <td>
                                  {c.rule}
                                  <small>{c.rulePayload}</small>
                                </td>
                                <td>{connectionRoute(c)}</td>
                                <td>
                                  {c.chains
                                    .filter(
                                      (v) =>
                                        ![
                                          "ATLAS",
                                          "AUTO",
                                          "FAILOVER",
                                          "DIRECT",
                                          "REJECT",
                                        ].includes(v),
                                    )
                                    .join(" → ") || "—"}
                                </td>
                              </tr>
                            ))}
                        </tbody>
                      </table>
                    </div>
                  ) : (
                    empty("Ожидаем трафик", "Откройте сайт или приложение.")
                  )}
                </>
              )}
              {page === "DNS" && s && (
                <>
                  {header(
                    "NAME RESOLUTION",
                    "Начните с правильного адреса.",
                    "Настройте DNS, который использует Mihomo.",
                  )}
                  <section className="settings-section">
                    <h2>DNS-серверы</h2>
                    <p>
                      IP-адреса, DNS over HTTPS или DNS over TLS. По одному
                      адресу на строку.
                    </p>
                    <textarea
                      value={dnsText}
                      onChange={(e) => setDnsText(e.target.value)}
                      rows={4}
                    />
                    <Toggle
                      title="IPv6"
                      text="Разрешить IPv6 в ядре и DNS"
                      value={s.dns.ipv6}
                      onChange={(v) =>
                        run(() => save({ ...s, dns: { ...s.dns, ipv6: v } }))
                      }
                    />
                    <Toggle
                      title="Fake-IP"
                      text="Сопоставлять домены с виртуальными IP для маршрутизации"
                      value={s.dns.fakeIp}
                      onChange={(v) =>
                        run(() => save({ ...s, dns: { ...s.dns, fakeIp: v } }))
                      }
                    />
                    <div className="actions">
                      <button
                        onClick={() =>
                          setDnsText(
                            "https://1.1.1.1/dns-query\nhttps://dns.google/dns-query",
                          )
                        }
                      >
                        По умолчанию
                      </button>
                      <button
                        className="primary"
                        disabled={busy}
                        onClick={() =>
                          run(() =>
                            save({
                              ...s,
                              dns: {
                                ...s.dns,
                                servers: dnsText
                                  .split("\n")
                                  .map((x) => x.trim())
                                  .filter(Boolean),
                              },
                            }),
                          )
                        }
                      >
                        Применить DNS
                      </button>
                    </div>
                    <p className="footnote">
                      В режиме системного прокси приложения могут выполнять
                      собственные DNS-запросы вне Mihomo. Этот режим не
                      обеспечивает системную защиту от DNS-утечек.
                    </p>
                  </section>
                </>
              )}
              {page === "Diagnostics" && (
                <>
                  {header(
                    "NETWORK HEALTH",
                    "Проверьте соединение.",
                    "Результаты реальных проверок ядра и HTTPS-доступности.",
                    <button
                      className="primary"
                      disabled={busy}
                      onClick={() =>
                        run(async () => {
                          setBusy(true);
                          try {
                            setChecks(await request("diagnostics"));
                          } catch (e) {
                            setError(String(e));
                          } finally {
                            setBusy(false);
                          }
                        })
                      }
                    >
                      <Stethoscope size={16} />
                      Запустить проверку
                    </button>,
                  )}
                  <section className="settings-section">
                    <h2>Отчёт для разбора сбоя</h2>
                    <p>Журналы Atlas, состояние службы и ядра, маршруты, DNS и DHCP-аренда этого компьютера. Работает без интернета. Сохраните отчёт до перезагрузки.</p>
                    <p className="footnote">Ключи и ссылки подписок скрываются. Локальные IP-адреса и названия адаптеров остаются в отчёте.</p>
                    <button disabled={reportBusy} onClick={async () => {
                      setReportBusy(true);
                      setReportSaved(false);
                      try {
                        const result = await request<{ saved: boolean }>("diagnostics_export");
                        setReportSaved(result.saved);
                      } catch (error) {
                        setError(String(error));
                      } finally {
                        setReportBusy(false);
                      }
                    }}>
                      {reportBusy ? "Собираем отчёт…" : "Скачать отчёт TXT"}
                    </button>
                    <p role="status">{reportSaved ? "Отчёт сохранён." : reportBusy ? "Выберите файл для сохранения. Сбор данных может занять до минуты." : ""}</p>
                  </section>
                  {checks.length ? (
                    <div className="list">
                      {checks.map((c) => (
                        <div className="diagnostic-row" data-result={c.ok === null ? "unknown" : c.ok ? "success" : "failure"} key={c.name}>
                          <span className={c.ok ? "checkmark" : "failed"}>
                            {c.ok === null ? (
                              "—"
                            ) : c.ok ? (
                              <Check size={19} />
                            ) : (
                              <X size={19} />
                            )}
                          </span>
                          <div>
                            <strong>{c.name}</strong>
                            <small>{c.detail}</small>
                          </div>
                          <span>
                            {c.ok === null
                              ? "Не проверено"
                              : c.ok
                                ? "Успешно"
                                : "Проблема"}
                          </span>
                        </div>
                      ))}
                    </div>
                  ) : (
                    empty(
                      "Понятный ответ вместо догадок",
                      "Проверьте ядро, API, подписки и доступность сайтов. Проверка обращается к Google, OpenAI и Telegram.",
                    )
                  )}
                </>
              )}
              {page === "Logs" && (
                <>
                  {header(
                    "ACTIVITY LOG",
                    "Что происходит внутри.",
                    "События приложения. URL подписок и учётные данные не записываются.",
                    <div className="actions">
                      <button
                        onClick={() => download("atlas-logs.json", data.logs)}
                      >
                        <Download size={16} />
                        Экспорт
                      </button>
                      <button onClick={() => run(() => act("clear_logs"))}>
                        Очистить
                      </button>
                    </div>,
                  )}
                  <div className="toolbar">
                    {search}
                    <select
                      value={logLevel}
                      onChange={(e) => setLogLevel(e.target.value)}
                    >
                      {["ALL", "INFO", "WARNING", "ERROR", "DEBUG"].map((l) => (
                        <option key={l} value={l}>
                          {{
                            ALL: "Все",
                            INFO: "Информация",
                            WARN: "Предупреждения",
                            ERROR: "Ошибки",
                          }[l] ?? l}
                        </option>
                      ))}
                    </select>
                  </div>
                  <div className="log-view">
                    {data.logs
                      .filter(
                        (l) =>
                          (logLevel === "ALL" || l.level === logLevel) &&
                          l.message.toLowerCase().includes(query.toLowerCase()),
                      )
                      .map((l, i) => (
                        <div key={i}>
                          <time>
                            {new Date(l.time * 1000).toLocaleTimeString()}
                          </time>
                          <span className={`log-${l.level}`}>
                            {{
                              INFO: "Информация",
                              WARN: "Предупреждение",
                              ERROR: "Ошибка",
                            }[l.level] ?? l.level}
                          </span>
                          <span>{l.message}</span>
                        </div>
                      ))}
                    {!data.logs.length && (
                      <p className="muted">Событий пока нет.</p>
                    )}
                  </div>
                </>
              )}
              {page === "Settings" && s && (
                <>
                  {header(
                    "MAKE IT YOURS",
                    "Всё под вашим контролем.",
                    "Настройки сохраняются локально и применяются после проверки.",
                  )}
                  <section className="settings-section">
                    <h2>Режим подключения</h2>
                    <select
                      aria-label="Режим подключения"
                      value={s.mode}
                      disabled={connected || busy}
                      onChange={(e) =>
                        run(() => save({ ...s, mode: e.target.value }))
                      }
                    >
                      <option value="tun">Вся система · TUN</option>
                    </select>
                  </section>
                  <section className="settings-section">
                    <h2>Внешний вид</h2>
                    <div className="setting-row">
                      <div>
                        <strong>Тема оформления</strong>
                        <p>Светлая, тёмная или системная</p>
                      </div>
                      <select
                        value={s.theme}
                        onChange={(e) =>
                          run(() => save({ ...s, theme: e.target.value }))
                        }
                      >
                        <option value="system">Системная</option>
                        <option value="light">Светлая</option>
                        <option value="dark">Тёмная</option>
                      </select>
                    </div>
                  </section>
                  <section className="settings-section">
                    <h2>Автопереключение</h2>
                    <div className="setting-row">
                      <div>
                        <strong>Интервал проверки серверов</strong>
                        <p>Как часто AUTO и FAILOVER измеряют доступность и задержку</p>
                      </div>
                      <select
                        aria-label="Интервал проверки серверов"
                        value={s.autoTestIntervalSeconds}
                        onChange={(e) =>
                          run(() =>
                            save({
                              ...s,
                              autoTestIntervalSeconds: Number(e.target.value),
                            }),
                          )
                        }
                      >
                        {autoTestIntervals.map((seconds) => (
                          <option key={seconds} value={seconds}>
                            {intervalLabel(seconds)}
                          </option>
                        ))}
                      </select>
                    </div>
                  </section>
                  <section className="settings-section">
                    <h2>Запуск</h2>
                    <Toggle
                      title="Запускать с Windows"
                      text="Автозапуск после входа в вашу учётную запись"
                      value={s.startup.launchWithWindows}
                      onChange={(v) =>
                        run(() =>
                          save({
                            ...s,
                            startup: { ...s.startup, launchWithWindows: v },
                          }),
                        )
                      }
                    />
                    <Toggle
                      title="Подключаться при запуске"
                      text="Запустить Mihomo и применить системный прокси"
                      value={s.startup.autoConnect}
                      onChange={(v) =>
                        run(() =>
                          save({
                            ...s,
                            startup: { ...s.startup, autoConnect: v },
                          }),
                        )
                      }
                    />
                    <Toggle
                      title="Запускать в трее"
                      text="Главное окно будет скрыто"
                      value={s.startup.startInTray}
                      onChange={(v) =>
                        run(() =>
                          save({
                            ...s,
                            startup: { ...s.startup, startInTray: v },
                          }),
                        )
                      }
                    />
                    <Toggle
                      title="Восстанавливать подключение"
                      text="Подключаться, если предыдущая сессия не была отключена"
                      value={s.startup.restoreConnection}
                      onChange={(v) =>
                        run(() =>
                          save({
                            ...s,
                            startup: { ...s.startup, restoreConnection: v },
                          }),
                        )
                      }
                    />
                    <div className="setting-row">
                      <div>
                        <strong>Задержка автоподключения</strong>
                        <p>Время для запуска сетевых служб Windows</p>
                      </div>
                      <select
                        value={s.startup.delaySeconds}
                        onChange={(e) =>
                          run(() =>
                            save({
                              ...s,
                              startup: {
                                ...s.startup,
                                delaySeconds: Number(e.target.value),
                              },
                            }),
                          )
                        }
                      >
                        {[0, 3, 5, 10, 15, 30].map((n) => (
                          <option key={n} value={n}>
                            {n} сек.
                          </option>
                        ))}
                      </select>
                    </div>
                  </section>
                  <section className="settings-section">
                    <h2>Резервные копии</h2>
                    <p>
                      Перед изменениями сохраняются предыдущие настройки. До 20
                      версий, защищённых Windows DPAPI.
                    </p>
                    <div className="actions">
                      <button
                        disabled={busy}
                        onClick={() => run(() => act("rollback"))}
                      >
                        <RefreshCw size={16} />
                        Восстановить предыдущую версию
                      </button>
                      <button
                        onClick={() =>
                          run(async () =>
                            download(
                              "atlas-settings.json",
                              await request("export"),
                            ),
                          )
                        }
                      >
                        <Download size={16} />
                        Экспорт без секретов
                      </button>
                    </div>
                  </section>
                  <section className="settings-section">
                    <h2>Об этой сборке</h2>
                    <p>Atlas {versionLabel} · Mihomo 1.19.31 · React + Tauri + Rust</p>
                  </section>
                </>
              )}
            </>
          )}
        </main>
        <footer className="main-footer">
          <span>
            <Shield size={13} />
            Локально. Приватно. Под вашим контролем.
          </span>
          <span>Ядро Mihomo</span>
        </footer>
      </div>
      {adding && (
        <div className="overlay">
          <section
            className="dialog"
            role="dialog"
            aria-modal="true"
            aria-label="Новая подписка"
          >
            <div className="section-head">
              <h2>Добавьте вашу подписку</h2>
              <button aria-label="Закрыть" onClick={() => setAdding(false)}>
                <X size={18} />
              </button>
            </div>
            <p>
              Atlas загрузит серверы по HTTPS и проверит их с помощью Mihomo.
            </p>
            <label>
              Название
              <input
                autoFocus
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder="Моя подписка"
              />
            </label>
            <label>
              Ссылка HTTPS
              <input
                type="password"
                autoComplete="off"
                spellCheck={false}
                value={url}
                onChange={(e) => setUrl(e.target.value)}
                placeholder="https://provider.example/sub/…"
              />
            </label>
            <p className="footnote">
              <Shield size={14} />
              Ссылка хранится в диспетчере учётных данных Windows. Никому не
              передавайте её.
            </p>
            {error && <p className="error">{error}</p>}
            <footer>
              <button onClick={() => setAdding(false)}>Отмена</button>
              <button
                disabled={busy || !url || !name.trim()}
                className="primary"
                onClick={() =>
                  run(async () => {
                    await act("subscription_add", { name, url });
                    setUrl("");
                    setName("");
                    setAdding(false);
                  })
                }
              >
                {busy ? (
                  <>
                    <LoaderCircle size={16} className="spin" />
                    Проверка…
                  </>
                ) : (
                  "Добавить подписку"
                )}
              </button>
            </footer>
          </section>
        </div>
      )}
      {inspection && (
        <div className="overlay">
          <section
            className="dialog"
            role="dialog"
            aria-label="Маршрут соединения"
            aria-modal="true"
          >
            <div className="section-head">
              <h2>Почему этот маршрут?</h2>
              <button aria-label="Закрыть" onClick={() => setInspection(null)}>
                <X size={18} />
              </button>
            </div>
            {Object.entries({
              Запрос:
                inspection.metadata.host || inspection.metadata.destinationIP,
              Процесс: inspection.metadata.process || "Не определён",
              Путь: inspection.metadata.processPath || "Не определён",
              "Сработавшее правило": `${inspection.rule}: ${inspection.rulePayload}`,
              Группа:
                s?.groups.find(
                  (g) =>
                    g.enabled &&
                    g.rules.some(
                      (r) =>
                        r.value === inspection.rulePayload &&
                        r.kind.replaceAll("-", "").toLowerCase() ===
                          inspection.rule.replaceAll("-", "").toLowerCase(),
                    ),
                )?.name || "По умолчанию",
              Маршрут: inspection.chains.join(" → "),
              Начало: new Date(inspection.start).toLocaleString(),
            }).map(([k, v]) => (
              <div className="setting-row" key={k}>
                <span className="muted">{k}</span>
                <strong>{v}</strong>
              </div>
            ))}
            {s && (
              <ConnectionRules
                connection={inspection}
                settings={s}
                save={save}
                onDone={() => {
                  setInspection(null);
                  setPage("Rules");
                }}
              />
            )}
            <footer>
              <button
                onClick={() =>
                  run(async () => {
                    await request("close_connection", { id: inspection.id });
                    setInspection(null);
                  })
                }
              >
                Закрыть соединение
              </button>
            </footer>
          </section>
        </div>
      )}
    </div>
  );
}
function Toggle({
  title,
  text,
  value,
  onChange,
}: {
  title: string;
  text: string;
  value: boolean;
  onChange: (v: boolean) => void;
}) {
  return (
    <div className="setting-row">
      <div>
        <strong>{title}</strong>
        <p>{text}</p>
      </div>
      <button
        role="switch"
        aria-checked={value}
        aria-label={title}
        className={`toggle ${value ? "on" : ""}`}
        onClick={() => onChange(!value)}
      >
        <span />
      </button>
    </div>
  );
}
createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
