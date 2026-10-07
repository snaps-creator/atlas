import type { SubscriptionSource } from "./types";

export function SubscriptionSourceSwitch({ value, pending, disabled, onChange }: {
  value: SubscriptionSource; pending: boolean; disabled: boolean;
  onChange: (source: SubscriptionSource) => void;
}) {
  return <section className="subscription-source" aria-label="Активный тип подписок" aria-busy={pending}>
    <div className="subscription-source-control" role="group" aria-label="Источник серверов">
      {(["URL", "VLESS"] as const).map(source => <button key={source} type="button"
        aria-pressed={source === value} disabled={disabled || pending}
        onClick={() => onChange(source)}>{source}</button>)}
    </div>
    <p role="status">{pending ? "Завершаем соединение и восстанавливаем сеть…" : "При смене источника VPN отключится. Подключение к новому серверу — вручную."}</p>
  </section>;
}
