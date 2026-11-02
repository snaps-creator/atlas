import { Activity } from "lucide-react";

export function ServerRefreshButton({ count, checking, onRefresh }: {
  count: number; checking: boolean; onRefresh: () => void;
}) {
  return <button disabled={count === 0} onClick={onRefresh}
    title={checking ? "Начать новую проверку всех серверов" : "Проверить задержку всех серверов"}>
    <Activity size={16} />
    {checking ? "Проверить заново" : "Проверить все"}
  </button>;
}
