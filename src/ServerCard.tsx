import { Globe2, Star } from "lucide-react";
import type { Server } from "./types";
import { latencyLabel, type Latency } from "./latency";
import "./servers.css";

export function ServerCard({ server, selected, favorite, latency, disabled, testing, onSelect, onFavorite, onTest }: {
  server: Server; selected: boolean; favorite: boolean; latency?: Latency;
  disabled: boolean; testing: boolean; onSelect: () => void; onFavorite: () => void; onTest: () => void;
}) {
  const delay = latency?.status === "ok" ? latency.delay : null;
  const tone = latency?.status === "unreachable" ? "timeout" : delay == null ? "unknown" : delay < 250 ? "fast" : delay < 400 ? "medium" : "slow";
  return <article className={`server-card ${selected ? "is-selected" : ""}`}>
    <button className="server-card-select" onClick={onSelect} disabled={disabled} aria-pressed={selected} title={server.name}>
      <span className="server-card-title">
        {server.country ? <span className={`fi fi-${server.country}`} role="img" aria-label={server.country.toUpperCase()} /> : <Globe2 size={18} />}
        <strong>{server.name}</strong>
      </span>
      <span className="server-card-tags"><span>{server.type.toUpperCase()}</span>{selected && <span>Выбран</span>}</span>
    </button>
    <button className={`server-card-delay ${tone}`} disabled={testing} onClick={onTest}
      aria-label={`Проверить задержку: ${server.name}. ${latencyLabel(latency)}`}
      title={latency?.error ?? "Задержка HTTP-запроса через сервер. При таймауте проверяется запасной адрес. Не скорость скачивания."}>
      {delay == null ? latencyLabel(latency) : <>{delay}<small>мс</small></>}
    </button>
    <button className="server-card-favorite" onClick={onFavorite} disabled={disabled} aria-pressed={favorite} aria-label={`${favorite ? "Убрать из избранного" : "В избранное"}: ${server.name}`}>
      <Star size={14} fill={favorite ? "currentColor" : "none"} />
    </button>
  </article>;
}
