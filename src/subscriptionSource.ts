import type { Settings, SubscriptionSource } from "./types";

export function activeSubscriptions(settings?: Settings | null) {
  const source = settings?.activeSource ?? "URL";
  return settings?.subscriptions.filter(sub => (sub.source ?? "URL") === source) ?? [];
}

export const sourceLabels: Record<SubscriptionSource, { title: string; empty: string; hint: string; input: string }> = {
  URL: { title: "URL-подписки", empty: "Нет серверов URL-подписок", hint: "Добавьте URL-подписку.", input: "Ссылка HTTPS" },
  VLESS: { title: "VLESS-подписки", empty: "Нет VLESS-серверов", hint: "Добавьте VLESS-подписку.", input: "Подписка HTTPS или ключ VLESS" },
};
