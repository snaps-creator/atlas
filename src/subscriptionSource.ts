import type { Server, Settings, SubscriptionSource } from "./types";

export function allSubscriptions(settings?: Pick<Settings, "subscriptions"> | null) {
  return settings?.subscriptions ?? [];
}

export const sourceLabels: Record<SubscriptionSource, { title: string; hint: string; input: string; placeholder: string }> = {
  URL: {
    title: "URL-подписка",
    hint: "Вставьте HTTPS-ссылку на подписку вашего провайдера.",
    input: "Ссылка HTTPS",
    placeholder: "https://provider.example/sub/…",
  },
  VLESS: {
    title: "VLESS-ключ",
    hint: "Вставьте один ключ, начинающийся с vless://.",
    input: "Ключ VLESS",
    placeholder: "vless://…",
  },
};

export function validSubscriptionInput(source: SubscriptionSource, input: string): boolean {
  try {
    const uri = new URL(input.trim());
    return !!uri.hostname && (source === "URL"
      ? uri.protocol === "https:"
      : uri.protocol === "vless:" && !!uri.username);
  } catch {
    return false;
  }
}

export function serverSelectionPayload(server: Server | "AUTO" | "FAILOVER") {
  if (typeof server === "string") return { nodeId: server };
  return server.nodeId ? { nodeId: server.nodeId } : { name: server.name };
}

export function isFavorite(favorites: string[], server: Server): boolean {
  return favorites.includes(server.nodeId || server.name);
}

export function toggleFavorite(favorites: string[], server: Server): string[] {
  const id = server.nodeId || server.name;
  return isFavorite(favorites, server) ? favorites.filter(value => value !== id) : [...favorites, id];
}

export function isServerSelected(settings: Pick<Settings, "selected" | "selectedNodeId"> | undefined, server: Server): boolean {
  return settings?.selectedNodeId
    ? settings.selectedNodeId === server.nodeId
    : settings?.selected === server.name;
}
