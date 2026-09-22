import { useEffect, useRef, useState } from "react";
import { download, request } from "./api";
import "./lan-diagnostics.css";

type Peer = {
  id: string;
  address: string;
  name: string;
  reported_name: string;
  last_seen: number;
};
type State = {
  id: string;
  name: string;
  enabled: boolean;
  peers: Peer[];
  listenerError?: string;
  port: number;
};
type Report = {
  deviceId: string;
  name: string;
  at: number;
  status?: string;
  selected?: string;
  version?: string;
  captureRunning?: boolean;
  captureCompletedAt?: number;
  history?: unknown;
};
export function LanDiagnostics() {
  const [configured, setConfigured] = useState(false),
    [password, setPassword] = useState(""),
    [token, setToken] = useState("");
  const [state, setState] = useState<State>(),
    [localName, setLocalName] = useState(""),
    [address, setAddress] = useState("");
  const [error, setError] = useState(""),
    [busy, setBusy] = useState(false),
    [report, setReport] = useState<Report>();
  const [cached, setCached] = useState(false),
    [online, setOnline] = useState<Record<string, boolean>>({});
  const liveToken = useRef("");
  const peers = useRef<Peer[]>([]);
  peers.current = state?.peers ?? [];
  useEffect(() => {
    void request<{ configured: boolean; loadError?: string }>("lan_info")
      .then((v) => {
        setConfigured(v.configured);
        if (v.loadError) setError(v.loadError);
      })
      .catch((e) => setError(String(e)));
    return () => {
      const t = liveToken.current;
      liveToken.current = "";
      if (t) void request("lan_lock", { token: t }).catch(() => {});
    };
  }, []);
  const call = async <T,>(action: string, payload: Record<string, unknown> = {}) => {
    const session = liveToken.current;
    const result = await request<T>(action, { ...payload, token: session });
    if (liveToken.current !== session) throw new Error("Просмотр закрыт");
    return result;
  };
  async function refresh(initial = false) {
    const t = liveToken.current;
    const s = await call<State>("lan_state");
    if (t !== liveToken.current) return;
    setState(s);
    if (initial) setLocalName(s.name);
  }
  async function run(task: () => Promise<void>) {
    setBusy(true);
    setError("");
    try {
      await task();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }
  async function fetchPeer(p: Peer, capture = false, display = true) {
    const t = liveToken.current;
    try {
      const r = await call<Report>("lan_fetch", {
        address: p.address,
        expectedId: p.id,
        capture,
        statusOnly: !display,
      });
      if (liveToken.current !== t) return;
      setOnline((v) => ({ ...v, [p.id]: true }));
      if (display) {
        setReport(r);
        setCached(false);
      }
    } catch (e) {
      if (liveToken.current === t) setOnline((v) => ({ ...v, [p.id]: false }));
      throw e;
    }
  }
  useEffect(() => {
    if (!token) return;
    const timer = setTimeout(() => {
      liveToken.current = "";
      setToken("");
      setReport(undefined);
      setState(undefined);
      setOnline({});
      setError("Срок доступа истёк. Введите пароль заново.");
      void request("lan_lock", { token }).catch(() => {});
    }, 1800000);
    return () => clearTimeout(timer);
  }, [token]);
  useEffect(() => {
    if (!token) return;
    let cancelled = false,
      pending = false;
    const poll = async () => {
      if (pending) return;
      pending = true;
      try {
        for (const p of peers.current) {
          if (cancelled) break;
          try {
            await fetchPeer(p, false, false);
          } catch {
            /* Last received reports remain available. */
          }
        }
        if (!cancelled) await refresh();
      } catch (e) {
        if (!cancelled) setError(String(e));
      } finally {
        pending = false;
      }
    };
    const timer = setInterval(() => {
      void poll();
    }, 30000);
    return () => {
      cancelled = true;
      clearInterval(timer);
    };
  }, [token]);
  if (!token)
    return (
      <section className="settings-section lan-diagnostics">
        <h2>Компьютеры в локальной сети</h2>
        <p>
          Просмотр диагностики других Atlas защищён паролем. Отдельный сервер не
          нужен.
        </p>
        <form
          onSubmit={(e) => {
            e.preventDefault();
            void run(async () => {
              const v = await request<{ token: string }>("lan_unlock", {
                password,
              });
              liveToken.current = v.token;
              setPassword("");
              setToken(v.token);
              setConfigured(true);
              await refresh(true);
            });
          }}
        >
          <label>
            {configured ? "Пароль" : "Задайте общий пароль на этом ПК"}
            <input
              type="password"
              autoComplete={configured ? "current-password" : "new-password"}
              minLength={8}
              maxLength={256}
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              required
            />
          </label>
          <button disabled={busy} type="submit">
            {busy
              ? "Проверяем…"
              : configured
                ? "Открыть"
                : "Сохранить пароль и открыть"}
          </button>
        </form>
        <p role="alert">{error}</p>
      </section>
    );
  return (
    <section className="settings-section lan-diagnostics">
      <div className="lan-heading">
        <h2>Компьютеры в локальной сети</h2>
        <button
          onClick={() => {
            const old = liveToken.current;
            liveToken.current = "";
            setToken("");
            setReport(undefined);
            setState(undefined);
            setOnline({});
            void request("lan_lock", { token: old }).catch(() => {});
          }}
        >
          Закрыть доступ
        </button>
      </div>
      <p>
        Одинаковый пароль нужен на каждом ПК. Отчёты зашифрованы; просмотр не
        меняет VPN. Доступ к панели действует 30 минут.
      </p>
      <div className="lan-controls">
        <label>
          Имя этого компьютера
          <input
            value={localName}
            maxLength={80}
            onChange={(e) => setLocalName(e.target.value)}
          />
        </label>
        <button
          disabled={busy || !state}
          onClick={() =>
            void run(async () => {
              await call("lan_save", {
                name: localName,
                enabled: state?.enabled,
              });
              await refresh();
            })
          }
        >
          Сохранить имя
        </button>
        <label className="lan-toggle">
          <input
            type="checkbox"
            checked={state?.enabled ?? false}
            disabled={busy || !state}
            onChange={(e) => {
              const enabled = e.target.checked;
              void run(async () => {
                await call("lan_save", { name: localName, enabled });
                await refresh();
              });
            }}
          />
          Разрешить другим Atlas читать диагностику этого ПК
        </label>
      </div>
      <p className="footnote">
        Для входящих подключений используется TCP {state?.port ?? 17943}, только
        локальная сеть. Компьютер должен быть включён, Atlas — запущен;
        подключение VPN не обязательно.
      </p>
      <button
        disabled={busy}
        onClick={() =>
          void run(async () => {
            const v = await call<{ message: string }>("lan_firewall");
            setError(v.message);
          })
        }
      >
        Разрешить приём в брандмауэре Windows
      </button>
      <p className="footnote">
        Windows запросит права администратора. Разрешение действует только для
        Atlas, частного профиля сети и локальной подсети.
      </p>
      {state?.listenerError && (
        <p role="alert">Не удалось включить приём: {state.listenerError}</p>
      )}
      <form
        onSubmit={(e) => {
          e.preventDefault();
          void run(async () => {
            const r = await call<Report>("lan_fetch", {
              address: address.trim(),
            });
            setReport(r);
            setCached(false);
            setOnline((v) => ({ ...v, [r.deviceId]: true }));
            setAddress("");
            await refresh();
          });
        }}
      >
        <label>
          Добавить компьютер по локальному IP
          <input
            placeholder="192.168.1.25"
            value={address}
            onChange={(e) => setAddress(e.target.value)}
            required
          />
        </label>
        <button disabled={busy}>Подключить</button>
      </form>
      <p role="alert">{error}</p>
      <div className="lan-peers">
        {state?.peers.map((p) => (
          <PeerRow
            key={p.id}
            peer={p}
            busy={busy}
            online={online[p.id]}
            onRename={(name) =>
              void run(async () => {
                await call("lan_rename", { id: p.id, name });
                await refresh();
              })
            }
            onFetch={() =>
              void run(async () => {
                await fetchPeer(p);
                await refresh();
              })
            }
            onCapture={() =>
              void run(async () => {
                await fetchPeer(p, true);
                await refresh();
              })
            }
            onCached={() =>
              void run(async () => {
                setReport(await call<Report>("lan_cached", { id: p.id }));
                setCached(true);
              })
            }
            onRemove={() =>
              void run(async () => {
                await call("lan_remove", { id: p.id });
                if (report?.deviceId === p.id) setReport(undefined);
                await refresh();
              })
            }
          />
        ))}
      </div>
      {!state?.peers.length && (
        <p>
          Компьютеры ещё не добавлены. Включите обмен в Atlas на другом ПК и
          укажите его IP.
        </p>
      )}
      {report && (
        <div className="lan-report">
          <h3>
            {state?.peers.find((p) => p.id === report.deviceId)?.name ??
              report.name}
          </h3>
          <p>
            {cached ? "Сохранённый отчёт" : "Полученный отчёт"} ·{" "}
            {new Date(report.at * 1000).toLocaleString()} · Atlas{" "}
            {report.version} · {report.status ?? "Нет состояния"}
          </p>
          <p>
            Выбранный сервер: {report.selected ?? "—"}.{" "}
            {report.captureRunning
              ? "Подробная диагностика собирается. Обновите отчёт примерно через минуту."
              : report.captureCompletedAt
                ? `Последний сбор: ${new Date(report.captureCompletedAt * 1000).toLocaleString()}`
                : ""}
          </p>
          <button
            onClick={() =>
              download(`atlas-lan-${report.deviceId}-${report.at}.json`, report)
            }
          >
            Скачать полученный отчёт
          </button>
          <details>
            <summary>История и подробности</summary>
            <pre>{JSON.stringify(report, null, 2)}</pre>
          </details>
        </div>
      )}
    </section>
  );
}
function PeerRow({
  peer,
  busy,
  online,
  onRename,
  onFetch,
  onCapture,
  onCached,
  onRemove,
}: {
  peer: Peer;
  busy: boolean;
  online?: boolean;
  onRename: (name: string) => void;
  onFetch: () => void;
  onCapture: () => void;
  onCached: () => void;
  onRemove: () => void;
}) {
  const [name, setName] = useState(peer.name);
  useEffect(() => setName(peer.name), [peer.name]);
  return (
    <article className="lan-peer">
      <label>
        Имя компьютера
        <input
          value={name}
          maxLength={80}
          onChange={(e) => setName(e.target.value)}
        />
      </label>
      <button
        disabled={busy || name === peer.name}
        onClick={() => onRename(name)}
      >
        Сохранить имя
      </button>
      <p>
        {peer.address} ·{" "}
        {online === true
          ? "На связи"
          : online === false
            ? "Нет связи"
            : "Ещё не проверен"}{" "}
        · Последний ответ: {new Date(peer.last_seen * 1000).toLocaleString()}
      </p>
      <div className="lan-actions">
        <button disabled={busy} onClick={onFetch}>
          Открыть отчёт
        </button>
        <button disabled={busy} onClick={onCapture}>
          Собрать диагностику сейчас
        </button>
        <button disabled={busy} onClick={onCached}>
          Последний сохранённый
        </button>
        <button disabled={busy} onClick={onRemove}>
          Убрать из списка
        </button>
      </div>
    </article>
  );
}
