import { useEffect, useRef, useState } from "react";
import { request } from "./api";
import { RouteSelect } from "./RuleEditor";
import type { Group, Subscription, SubscriptionSource } from "./types";

type Peer = { id: string; name: string; address: string };
type Remote = { revision: number; status: string; groups: Group[]; subscriptions: Subscription[] };
type Reply = { value: Remote; allowChanges: boolean };
export function LanManagement({ peer, token, onClose }: { peer: Peer; token: string; onClose: () => void }) {
  const [data, setData] = useState<Reply>();
  const [groups, setGroups] = useState<Group[]>([]);
  const [busy, setBusy] = useState(false), [error, setError] = useState("");
  const [message, setMessage] = useState("");
  const [query, setQuery] = useState("");
  const [source, setSource] = useState<SubscriptionSource>("URL");
  const [url, setUrl] = useState(""), [name, setName] = useState("");
  const pending = useRef(false), live = useRef(true);
  useEffect(() => { live.current = true; void perform("view"); return () => { live.current = false; }; }, []);
  async function perform(operation: string, payload: Record<string, unknown> = {}) {
    if (pending.current) return;
    pending.current = true; setBusy(true); setError(""); setMessage("");
    try {
      const reply = await request<Reply>("lan_manage", { token, address: peer.address, expectedId: peer.id,
        operation, payload: { ...payload, revision: data?.value.revision } });
      if (!live.current) return;
      setData(reply);
      if (operation === "view" || operation === "rules_save") setGroups(structuredClone(reply.value.groups));
      if (operation === "subscription_add") { setUrl(""); setName(""); }
      if (operation !== "view") setMessage("Изменения сохранены на «" + peer.name + "».");
    } catch (e) { if (live.current) setError(String(e)); }
    finally { pending.current = false; if (live.current) setBusy(false); }
  }
  const writable = data?.allowChanges === true;
  const disabled = busy || !writable;
  const dirty = JSON.stringify(groups) !== JSON.stringify(data?.value.groups ?? []);
  function updateGroup(id: string, change: Partial<Group>) { setGroups(current => current.map(g => g.id === id ? { ...g, ...change } : g)); }
  return <section className="lan-management" aria-label={`Настройки ${peer.name}`}>
    <div className="lan-heading"><h3>Правила и подписки · {peer.name}</h3><button onClick={onClose} disabled={busy}>Закрыть настройки</button></div>
    <p>{peer.address} · {data?.value.status ?? "Подключение…"}</p>
    {data && !writable && <p>Просмотр доступен. Для изменений включите «Разрешить изменение настроек по локальной сети» в Atlas на этом ПК.</p>}
    <button disabled={busy} onClick={() => {
      if (!dirty || window.confirm("Сбросить несохранённые изменения правил и получить текущие настройки?")) void perform("view");
    }}>{busy ? "Выполняется…" : "Обновить настройки с ПК"}</button>
    <p role="alert">{error}</p><p role="status">{message}</p>
    {data && <>
      <h4>Правила ({data.value.groups.reduce((n, g) => n + g.rules.length, 0)})</h4>
      <label>Найти правило<input value={query} onChange={e => setQuery(e.target.value)} /></label>
      <fieldset disabled={disabled}>
        {groups.filter(g => !query || `${g.name} ${g.rules.map(r => r.value).join(" ")}`.toLowerCase().includes(query.toLowerCase())).map(g => <details key={g.id}>
          <summary>{g.name} · {g.rules.length} · {g.enabled ? "Включено" : "Выключено"}</summary>
          <div className="lan-actions">
            <label>Название группы<input maxLength={128} value={g.name} onChange={e => updateGroup(g.id, { name: e.target.value })} /></label>
            <label>Маршрут<RouteSelect value={g.route} onChange={route => updateGroup(g.id, { route })} /></label>
            <label className="lan-toggle"><input type="checkbox" checked={g.enabled} onChange={e => updateGroup(g.id, { enabled: e.target.checked })} />Включена</label>
          </div>
          {g.rules.map((rule, index) => <div className="lan-actions lan-rule" key={index}>
            <label>Тип правила<input list="lan-rule-kinds" value={rule.kind} onChange={e => updateGroup(g.id, { rules: g.rules.map((r, i) => i === index ? { ...r, kind: e.target.value } : r) })} /></label>
            <label className="lan-rule-value">Значение<input value={rule.value} onChange={e => updateGroup(g.id, { rules: g.rules.map((r, i) => i === index ? { ...r, value: e.target.value } : r) })} /></label>
            <label className="lan-toggle"><input type="checkbox" checked={rule.noResolve} onChange={e => updateGroup(g.id, { rules: g.rules.map((r, i) => i === index ? { ...r, noResolve: e.target.checked } : r) })} />Без DNS</label>
            <button onClick={() => updateGroup(g.id, { rules: g.rules.filter((_, i) => i !== index) })}>Удалить правило</button>
          </div>)}
          <div className="lan-actions"><button onClick={() => updateGroup(g.id, { rules: [...g.rules, { kind: "DOMAIN-SUFFIX", value: "", noResolve: false }] })}>Добавить правило</button>
            <button onClick={() => { if (window.confirm(`Удалить группу «${g.name}» и её правила?`)) setGroups(groups.filter(x => x.id !== g.id)); }}>Удалить группу</button></div>
        </details>)}
        <datalist id="lan-rule-kinds">{["DOMAIN", "DOMAIN-SUFFIX", "DOMAIN-KEYWORD", "IP-CIDR", "IP-CIDR6", "PROCESS-NAME", "PROCESS-PATH", "DST-PORT", "GEOIP", "GEOSITE"].map(kind => <option value={kind} key={kind} />)}</datalist>
        <div className="lan-actions"><button onClick={() => setGroups([...groups, { id: crypto.randomUUID(), name: "Новая группа", description: "", enabled: true, route: "PROXY", rules: [{ kind: "DOMAIN-SUFFIX", value: "", noResolve: false }] }])}>Добавить группу</button>
          <button disabled={!dirty} onClick={() => void perform("rules_save", { groups })}>Сохранить правила на ПК</button></div>
      </fieldset>
      <h4>Подписки ({data.value.subscriptions.length})</h4>
      {data.value.subscriptions.map(sub => <RemoteSubscription key={sub.id} sub={sub} disabled={disabled} busy={busy}
        perform={perform} />)}
      <form onSubmit={e => { e.preventDefault(); void perform("subscription_add", { source, url: url.trim(), name: name.trim() || "Подписка" }); }}>
        <fieldset disabled={disabled}>
          <legend>Добавить подписку на {peer.name}</legend>
          <label>Тип<select value={source} onChange={e => setSource(e.target.value as SubscriptionSource)}><option value="URL">HTTPS-подписка</option><option value="VLESS">VLESS-ключ</option></select></label>
          <label>Название<input value={name} onChange={e => setName(e.target.value)} maxLength={128} /></label>
          <label>{source === "URL" ? "Ссылка HTTPS" : "Ключ VLESS"}<input type="password" autoComplete="off" value={url} onChange={e => setUrl(e.target.value)} required /></label>
          <button type="submit">Добавить на ПК</button>
        </fieldset>
      </form>
    </>}
  </section>;
}
function RemoteSubscription({ sub, disabled, busy, perform }: { sub: Subscription; disabled: boolean; busy: boolean; perform: (operation: string, payload?: Record<string, unknown>) => Promise<void> }) {
  const [name, setName] = useState(sub.name), [url, setUrl] = useState("");
  const [agent, setAgent] = useState(sub.options?.userAgentOverride ?? "");
  useEffect(() => { setName(sub.name); setAgent(sub.options?.userAgentOverride ?? ""); }, [sub]);
  return <details><summary>{sub.name} · {sub.servers.length} серверов</summary>
    <p>{sub.maskedUrl} · {sub.updatedAt ? `Обновлена ${new Date(sub.updatedAt * 1000).toLocaleString()}` : "Ещё не обновлялась"}</p>
    {sub.error && <p role="alert">{sub.error}</p>}
    <fieldset disabled={disabled}>
      <label>Название<input maxLength={128} value={name} onChange={e => setName(e.target.value)} /></label>
      <button disabled={!name.trim() || name === sub.name} onClick={() => void perform("subscription_rename", { id: sub.id, name: name.trim() })}>Сохранить название</button>
      <label>Новая ссылка или ключ<input type="password" autoComplete="off" value={url} onChange={e => setUrl(e.target.value)} /></label>
      <button disabled={!url.trim()} onClick={async () => { const next = url.trim(); setUrl(""); await perform("subscription_replace", { id: sub.id, url: next }); }}>Заменить и загрузить</button>
      {sub.source !== "VLESS" && <>
        <label>User-Agent<input value={agent} maxLength={512} onChange={e => setAgent(e.target.value)} /></label>
        <button disabled={agent === (sub.options?.userAgentOverride ?? "")} onClick={() => void perform("subscription_user_agent", { id: sub.id, userAgent: agent })}>Сохранить User-Agent</button>
        <button onClick={() => void perform("subscription_refresh", { id: sub.id })}>{busy ? "Обновляется…" : "Обновить подписку"}</button>
      </>}
      <button onClick={() => { if (window.confirm(`Удалить подписку «${sub.name}» на удалённом ПК?`)) void perform("subscription_delete", { id: sub.id }); }}>Удалить подписку</button>
    </fieldset>
  </details>;
}
