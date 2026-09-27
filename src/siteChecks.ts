export type SiteResult = {
  ok: boolean;
  ms: number | null;
  status: number | null;
  error?: string;
  errorKind?: "local" | "timeout" | "network";
};

export function siteResultLabel(result: SiteResult): string {
  if (result.status != null) return result.ok
    ? `HTTP-ответ: ${result.ms} мс` : `HTTP ${result.status}`;
  if (result.errorKind === "timeout") return "Таймаут запроса";
  if (result.errorKind === "network") return "Ошибка соединения";
  return "Проверка не выполнена";
}
