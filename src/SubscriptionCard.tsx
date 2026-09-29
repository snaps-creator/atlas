import React, { useEffect, useRef, useState } from "react";
import { RefreshCw, Trash2 } from "lucide-react";
import type { Subscription } from "./types";
import { subscriptionAge } from "./subscriptionAge";

export function SubscriptionCard({ sub, refreshing, canDelete, refresh, remove, saveUserAgent }: {
  sub: Subscription; refreshing: boolean; canDelete: boolean;
  refresh: () => Promise<unknown>; remove: () => void;
  saveUserAgent: (agent: string) => Promise<unknown>;
}) {
  const [now, setNow] = useState(Date.now());
  const [pending, setPending] = useState(false);
  const [error, setError] = useState("");
  const [agent, setAgent] = useState(sub.options?.userAgentOverride ?? "");
  const [savingAgent, setSavingAgent] = useState(false);
  useEffect(() => setAgent(sub.options?.userAgentOverride ?? ""), [sub.options?.userAgentOverride]);
  const inFlight = useRef(false);
  useEffect(() => setError(""), [sub.updatedAt]);
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), 15000);
    return () => clearInterval(timer);
  }, []);
  const busy = pending || refreshing;
  async function update() {
    if (inFlight.current || refreshing) return;
    inFlight.current = true; setPending(true); setError("");
    try { await refresh(); } catch (e) { setError(String(e)); }
    finally { inFlight.current = false; setPending(false); setNow(Date.now()); }
  }
  return <div className="subscription">
    <div className="section-head">
      <div><h2>{sub.name}</h2><p>{sub.maskedUrl}</p></div>
      <div className="actions">
        <button disabled={busy} onClick={() => void update()}><RefreshCw size={15} />{busy ? "Обновляется…" : "Обновить"}</button>
        <button aria-label="Удалить подписку" disabled={busy || !canDelete} onClick={remove}><Trash2 size={16} /></button>
      </div>
    </div>
    <div className="subscription-meta">
      <span>{sub.servers.length} серверов</span>
      <span title={sub.updatedAt > 0 ? new Date(sub.updatedAt * 1000).toLocaleString("ru") : undefined}>
        {sub.updatedAt > 0 ? `Обновлена ${subscriptionAge(sub.updatedAt, now)}` : "Ещё не обновлялась"}
      </span>
    </div>
    <details>
      <summary>Настройки загрузки подписки</summary>
      <label htmlFor={`agent-${sub.id}`}>User-Agent</label>
      <div className="actions">
        <input id={`agent-${sub.id}`} value={agent} maxLength={512}
          placeholder={sub.options?.userAgent ?? "Happ/4.3.0"}
          onChange={e => setAgent(e.target.value)} disabled={savingAgent} />
        <button disabled={savingAgent || agent === (sub.options?.userAgentOverride ?? "")}
          onClick={async () => {
            setSavingAgent(true); setError("");
            try { await saveUserAgent(agent); } catch (e) { setError(String(e)); }
            finally { setSavingAgent(false); }
          }}>{savingAgent ? "Сохраняется…" : "Сохранить"}</button>
      </div>
      <p>Пустое поле — настройки провайдера или совместимый User-Agent Happ. Изменение применяется при следующем обновлении этой подписки.</p>
    </details>
    {(error || sub.error) && <p className="error">{error || sub.error}</p>}
  </div>;
}
